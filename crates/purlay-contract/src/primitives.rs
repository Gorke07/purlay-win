use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// An id on the wire. Always lower-case text and compared as a string: UUIDv7 ids sort
/// by time that way, which the realtime cursors rely on.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Uuid(String);

impl Uuid {
    pub fn new(s: impl AsRef<str>) -> Self {
        Uuid(s.as_ref().to_ascii_lowercase())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Whether the text has the 8-4-4-4-12 hex shape.
    pub fn is_shape(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 36
            && b.iter().enumerate().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => *c == b'-',
                _ => c.is_ascii_hexdigit(),
            })
    }
}

impl std::ops::Deref for Uuid {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

// Hash, Eq and Ord are the inner String's, so maps keyed by `Uuid` can be read with `&str`.
impl std::borrow::Borrow<str> for Uuid {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Uuid {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Uuid {
    fn from(s: &str) -> Self {
        Uuid::new(s)
    }
}

impl From<String> for Uuid {
    fn from(s: String) -> Self {
        Uuid::new(s)
    }
}

impl Serialize for Uuid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<'de, str>::deserialize(d)?;
        Ok(Uuid::new(&*s))
    }
}

/// An ISO 8601 instant in UTC with milliseconds, as the server writes it
/// (`2026-11-10T09:30:00.000Z`).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn new(s: impl Into<String>) -> Self {
        Timestamp(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Milliseconds since the Unix epoch, if the text parses.
    pub fn unix_ms(&self) -> Option<i64> {
        parse_iso_ms(&self.0)
    }
    /// Formats milliseconds since the epoch the way the server does.
    pub fn from_unix_ms(ms: i64) -> Self {
        Timestamp(format_iso_ms(ms))
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `{}` with nothing allowed in it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EmptyObject {}

/// An object whose shape the contract leaves open.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

// --- civil-time helpers (no time-zone database needed: the wire is UTC) ---

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn parse_iso_ms(s: &str) -> Option<i64> {
    // YYYY-MM-DDTHH:MM:SS(.fff)?(Z|±HH:MM)
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't' && b[10] != b' ') {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &s[start..i];
        let mut digits: String = frac.chars().take(3).collect();
        while digits.len() < 3 {
            digits.push('0');
        }
        ms = digits.parse().ok()?;
    }
    let offset_min = match b.get(i)? {
        b'Z' | b'z' => 0,
        b'+' | b'-' => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            let oh = num(i + 1, i + 3)?;
            let om = num(i + 4, i + 6)?;
            sign * (oh * 60 + om)
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 24 + h) * 60 + mi - offset_min) * 60_000 + se * 1000 + ms)
}

pub fn format_iso_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = (rem / 60_000) % 60;
    let s = (rem / 1000) % 60;
    let f = rem % 1000;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{f:03}Z")
}
