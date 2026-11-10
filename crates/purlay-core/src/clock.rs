//! Local and server-corrected clocks (D-33).

use std::sync::Arc;

/// Milliseconds since the Unix epoch.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

pub type SharedClock = Arc<dyn Clock>;

/// The clock message ids are made against: the local clock plus the offset measured from
/// the first `PONG` (`offset = server_ts − (sent_at + rtt/2)`). Later samples are ignored.
#[derive(Debug, Default, Clone)]
pub struct ServerClock {
    offset_ms: Option<f64>,
}

impl ServerClock {
    pub fn corrected(&self) -> bool {
        self.offset_ms.is_some()
    }

    pub fn offset_ms(&self) -> f64 {
        self.offset_ms.unwrap_or(0.0)
    }

    pub fn now_ms(&self, local_ms: i64) -> i64 {
        (local_ms as f64 + self.offset_ms()).round() as i64
    }

    /// One PING/PONG exchange. A reply that arrived before its request left is refused.
    pub fn observe(&mut self, sent_at_ms: i64, received_at_ms: i64, server_ts: f64) -> Result<(), &'static str> {
        let round_trip = received_at_ms - sent_at_ms;
        if round_trip < 0 {
            return Err("a PONG cannot arrive before its PING was sent");
        }
        if self.offset_ms.is_some() {
            return Ok(());
        }
        self.offset_ms = Some(server_ts - (sent_at_ms as f64 + round_trip as f64 / 2.0));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_from_the_midpoint() {
        let mut c = ServerClock::default();
        assert!(!c.corrected());
        c.observe(1_000, 1_200, 6_100.0).unwrap();
        assert_eq!(c.offset_ms(), 5_000.0);
        assert_eq!(c.now_ms(2_000), 7_000);
        // Later samples are ignored.
        c.observe(3_000, 3_010, 0.0).unwrap();
        assert_eq!(c.offset_ms(), 5_000.0);
    }

    #[test]
    fn refuses_negative_round_trip() {
        let mut c = ServerClock::default();
        assert!(c.observe(10, 5, 0.0).is_err());
        assert!(!c.corrected());
    }
}
