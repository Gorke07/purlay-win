//! Message ids: UUIDv7 made by the client against the server-corrected clock (D-8, D-33).
//!
//! `message_id(ms, random)` lays out 48 bits of time, version 7, the first 12 random bits,
//! variant `10`, then the last 62 of the 80 random bits — the server's `messageId`, checked
//! against the bundle's `fixtures/message-ids.json`.

use purlay_contract::constants::message_id_skew;
use purlay_contract::Uuid;

/// The id for `timestamp_ms` and 80 random bits.
pub fn message_id(timestamp_ms: u64, random: u128) -> Uuid {
    let ts = timestamp_ms & 0xffff_ffff_ffff;
    let random = random & ((1u128 << 80) - 1);
    let rand_a = (random >> 68) as u64 & 0xfff;
    let rand_b = random & ((1u128 << 62) - 1);
    let hi: u64 = (ts << 16) | (0x7 << 12) | rand_a;
    let lo: u64 = (0b10u64 << 62) | rand_b as u64;
    let hex = format!("{hi:016x}{lo:016x}");
    Uuid::new(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
}

/// The shape the server accepts: lower-case hex, hyphens, version 7, variant 10.
pub fn is_message_id_shape(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let ok = match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        };
        if !ok {
            return false;
        }
    }
    b[14] == b'7' && matches!(b[19], b'8' | b'9' | b'a' | b'b')
}

/// The milliseconds an id was made at.
pub fn timestamp_of(id: &str) -> Option<u64> {
    if id.len() < 13 {
        return None;
    }
    let hex: String = id[0..8].chars().chain(id[9..13].chars()).collect();
    u64::from_str_radix(&hex, 16).ok()
}

/// Whether the server would take this id now (`MESSAGE_ID_SKEW`).
pub fn is_plausible(id: &str, now_ms: i64) -> bool {
    let Some(ts) = timestamp_of(id) else { return false };
    let ts = ts as i64;
    ts >= now_ms - message_id_skew::BEHIND_MS && ts <= now_ms + message_id_skew::AHEAD_MS
}

/// Makes ids that strictly increase within a run, even within one millisecond.
#[derive(Debug, Default)]
pub struct IdGen {
    last_ms: u64,
    /// The 74 bits that reach the id: 12 of `rand_a`, then 62 of `rand_b`.
    last_bits: u128,
}

impl IdGen {
    pub fn next(&mut self, now_ms: i64, random: u128) -> Uuid {
        let now = now_ms.max(0) as u64;
        const BITS74: u128 = (1u128 << 74) - 1;
        if self.last_ms != 0 && now <= self.last_ms {
            // Same (or an earlier) millisecond: one more than the last, so ids still rise.
            self.last_bits = (self.last_bits + 1) & BITS74;
            if self.last_bits == 0 {
                self.last_ms += 1;
            }
        } else {
            self.last_ms = now;
            // Leave headroom so the steps within one millisecond do not wrap.
            self.last_bits = (random & BITS74) >> 1;
        }
        let rand_a = self.last_bits >> 62;
        let rand_b = self.last_bits & ((1u128 << 62) - 1);
        message_id(self.last_ms, (rand_a << 68) | rand_b)
    }
}

pub fn random_u128() -> u128 {
    rand::random::<u128>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Option<serde_json::Value> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Contract/bundle/purlay-contract-v0.6.1/fixtures/message-ids.json");
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    #[test]
    fn generated_vectors() {
        let Some(f) = fixture() else { return };
        for case in f["generated"].as_array().unwrap() {
            let ms = case["timestampMs"].as_u64().unwrap();
            let r = u128::from_str_radix(case["randomHex"].as_str().unwrap(), 16).unwrap();
            assert_eq!(message_id(ms, r).as_str(), case["id"].as_str().unwrap(), "{case}");
        }
    }

    #[test]
    fn shape_vectors() {
        let Some(f) = fixture() else { return };
        for case in f["shape"].as_array().unwrap() {
            assert_eq!(is_message_id_shape(case["value"].as_str().unwrap()), case["valid"].as_bool().unwrap(), "{case}");
        }
    }

    #[test]
    fn plausibility_vectors() {
        let Some(f) = fixture() else { return };
        for case in f["plausibility"].as_array().unwrap() {
            let now = case["nowMs"].as_i64().unwrap();
            assert_eq!(is_plausible(case["id"].as_str().unwrap(), now), case["plausible"].as_bool().unwrap(), "{case}");
        }
    }

    #[test]
    fn ids_increase_within_a_millisecond() {
        let mut g = IdGen::default();
        let mut last = g.next(1_000, u128::MAX);
        for _ in 0..1000 {
            let next = g.next(1_000, 0);
            assert!(next > last, "{next} <= {last}");
            assert!(is_message_id_shape(&next));
            last = next;
        }
        let later = g.next(1_001, 5);
        assert!(later > last);
    }
}
