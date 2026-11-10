//! Ids and clocks.
//!
//! Every id the server makes is a lower-case UUIDv7, so ids compare by time as strings,
//! which is what the keyset cursors and the socket replay rely on. The layout is the
//! contract's (`fixtures/message-ids.json`): 48 bits of milliseconds, version 7, 12 random
//! bits, variant `10`, 62 random bits.

use parking_lot::Mutex;
use purlay_contract::Uuid;
use purlay_contract::constants::message_id_skew;
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, by the wall clock.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The `ts` every server frame carries.
pub fn now_ts() -> f64 {
    now_ms() as f64
}

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

/// The smallest id of a millisecond: everything made at or after `ms` sorts above it.
pub fn floor_id(ms: i64) -> Uuid {
    message_id(ms.max(0) as u64, 0)
}

/// An id for an instant in the past or future (seed data), with fresh random bits.
pub fn id_at(ms: i64) -> Uuid {
    message_id(ms.max(0) as u64, rand::random::<u128>())
}

struct Gen {
    last_ms: u64,
    last_bits: u128,
}

static GEN: Mutex<Gen> = parking_lot::const_mutex(Gen { last_ms: 0, last_bits: 0 });

/// A fresh id for now that is strictly greater than every id this process made before
/// with `new_id`, even within one millisecond.
pub fn new_id() -> Uuid {
    const BITS74: u128 = (1u128 << 74) - 1;
    let now = now_ms().max(0) as u64;
    let mut g = GEN.lock();
    if g.last_ms != 0 && now <= g.last_ms {
        g.last_bits = (g.last_bits + 1) & BITS74;
        if g.last_bits == 0 {
            g.last_ms += 1;
        }
    } else {
        g.last_ms = now;
        g.last_bits = (rand::random::<u128>() & BITS74) >> 1;
    }
    let rand_a = g.last_bits >> 62;
    let rand_b = g.last_bits & ((1u128 << 62) - 1);
    message_id(g.last_ms, (rand_a << 68) | rand_b)
}

/// Lower-case hex, hyphens, version 7, variant `10`.
pub fn is_v7_shape(s: &str) -> bool {
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

/// Any UUID text (any version), as a path or query parameter is checked.
pub fn is_uuid(s: &str) -> bool {
    Uuid::is_shape(s)
}

/// The milliseconds a v7 id was made at.
pub fn timestamp_of(id: &str) -> Option<i64> {
    if id.len() < 13 {
        return None;
    }
    let hex: String = id[0..8].chars().chain(id[9..13].chars()).collect();
    u64::from_str_radix(&hex, 16).ok().map(|v| v as i64)
}

/// Whether a client-made message id is inside the skew window (`MESSAGE_ID_SKEW`).
pub fn is_plausible(id: &str, now_ms: i64) -> bool {
    let Some(ts) = timestamp_of(id) else { return false };
    ts >= now_ms - message_id_skew::BEHIND_MS && ts <= now_ms + message_id_skew::AHEAD_MS
}

/// `n` random bytes as lower-case hex.
pub fn random_hex(n: usize) -> String {
    let mut out = String::with_capacity(n * 2);
    for _ in 0..n {
        out.push_str(&format!("{:02x}", rand::random::<u8>()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_rise_and_have_the_shape() {
        let mut last = new_id();
        for _ in 0..2000 {
            let next = new_id();
            assert!(next > last);
            assert!(is_v7_shape(&next));
            last = next;
        }
        let at = id_at(1_700_000_000_000);
        assert_eq!(timestamp_of(&at), Some(1_700_000_000_000));
        assert!(floor_id(1_700_000_000_000) <= at);
    }

    #[test]
    fn fixture_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Contract/bundle/purlay-contract-v0.6.1/fixtures/message-ids.json");
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let f: serde_json::Value = serde_json::from_str(&text).unwrap();
        for case in f["generated"].as_array().unwrap() {
            let ms = case["timestampMs"].as_u64().unwrap();
            let r = u128::from_str_radix(case["randomHex"].as_str().unwrap(), 16).unwrap();
            assert_eq!(message_id(ms, r).as_str(), case["id"].as_str().unwrap());
        }
        for case in f["plausibility"].as_array().unwrap() {
            let now = case["nowMs"].as_i64().unwrap();
            assert_eq!(is_plausible(case["id"].as_str().unwrap(), now), case["plausible"].as_bool().unwrap());
        }
    }
}
