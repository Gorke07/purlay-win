use purlay_contract::constants::backoff;

/// `attempt` is the number of failures so far: 0 for the first retry.
///
/// The delay is in the upper half of the exponential step, never near zero: full jitter
/// would let one client retry immediately, several times in a row, against a server that is
/// refusing it. `random` is in `[0, 1)`.
pub fn reconnect_delay(attempt: u32, random: f64) -> u64 {
    let base = backoff::BASE_MS as f64;
    let cap = backoff::CAP_MS as f64;
    let step = cap.min(base * 2f64.powi(attempt.min(30) as i32));
    (step / 2.0 + random * step / 2.0).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_half_of_each_step() {
        assert_eq!(reconnect_delay(0, 0.0), 500);
        assert_eq!(reconnect_delay(0, 1.0), 1000);
        assert_eq!(reconnect_delay(1, 0.0), 1000);
        assert_eq!(reconnect_delay(1, 0.5), 1500);
        assert_eq!(reconnect_delay(3, 0.0), 4000);
    }

    #[test]
    fn capped() {
        assert_eq!(reconnect_delay(10, 0.0), 15000);
        assert_eq!(reconnect_delay(10, 1.0), 30000);
        assert_eq!(reconnect_delay(1000, 1.0), 30000);
    }
}
