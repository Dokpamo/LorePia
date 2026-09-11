use std::{io, time::Instant};

const BURST_BYTES: u64 = 64 * 1024 * 1024;
const BYTES_PER_SECOND: u64 = 8 * 1024 * 1024;
const MIN_CHARGE_BYTES: u64 = 128 * 1024;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Bounds both hash I/O and tiny-file overhead without a minute-long count cliff.
pub(super) struct VerificationBudget {
    byte_nanos: u128,
    updated_at: Instant,
}

impl VerificationBudget {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            byte_nanos: u128::from(BURST_BYTES) * NANOS_PER_SECOND,
            updated_at: now,
        }
    }

    pub(super) fn admit(&mut self, size_bytes: u64, now: Instant) -> io::Result<()> {
        if size_bytes > BURST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "asset exceeds the bounded verification size",
            ));
        }
        let now = now.max(self.updated_at);
        let replenished = now
            .duration_since(self.updated_at)
            .as_nanos()
            .saturating_mul(u128::from(BYTES_PER_SECOND));
        self.byte_nanos = self
            .byte_nanos
            .saturating_add(replenished)
            .min(u128::from(BURST_BYTES) * NANOS_PER_SECOND);
        self.updated_at = now;
        let charge = u128::from(size_bytes.max(MIN_CHARGE_BYTES)) * NANOS_PER_SECOND;
        if charge > self.byte_nanos {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "verified asset hash budget is temporarily exhausted",
            ));
        }
        self.byte_nanos -= charge;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn remaining(&self) -> u128 {
        self.byte_nanos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn small_album_does_not_share_the_cost_of_full_size_media() {
        let now = Instant::now();
        let mut budget = VerificationBudget::new(now);
        for index in 0..278 {
            budget
                .admit(81_024, now)
                .unwrap_or_else(|error| panic!("album image {index}: {error}"));
        }
        assert_eq!(
            budget.remaining(),
            u128::from(BURST_BYTES - 278 * MIN_CHARGE_BYTES) * NANOS_PER_SECOND
        );
    }

    #[test]
    fn tiny_files_and_large_files_both_have_a_finite_burst() {
        let now = Instant::now();
        let mut budget = VerificationBudget::new(now);
        for _ in 0..BURST_BYTES / MIN_CHARGE_BYTES {
            budget.admit(1, now).expect("bounded tiny-file burst");
        }
        assert_eq!(
            budget.admit(1, now).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let mut budget = VerificationBudget::new(now);
        budget
            .admit(BURST_BYTES, now)
            .expect("one maximum-size asset");
        assert_eq!(
            budget.admit(1, now).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn rejected_retries_do_not_restart_the_refill_or_lose_fractional_credit() {
        let now = Instant::now();
        let mut budget = VerificationBudget::new(now);
        budget.admit(BURST_BYTES, now).expect("consume burst");
        // A 64MiB asset is eligible after eight seconds, even with a failed
        // request every millisecond. Failures must not reset a whole window.
        for millis in 1..8000 {
            assert_eq!(
                budget
                    .admit(BURST_BYTES, now + Duration::from_millis(millis))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
        }
        budget
            .admit(BURST_BYTES, now + Duration::from_secs(8))
            .expect("continuous refill");
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn tiny_requests_recover_without_waiting_for_a_full_large_asset() {
        let now = Instant::now();
        let mut budget = VerificationBudget::new(now);
        budget.admit(BURST_BYTES, now).expect("consume burst");
        budget
            .admit(1, now + Duration::from_millis(16))
            .expect("small request refilled");
        assert!(
            budget
                .admit(BURST_BYTES, now + Duration::from_millis(16))
                .is_err()
        );
    }

    #[test]
    fn idle_time_cannot_accumulate_unlimited_credit_and_oversize_is_permanent() {
        let now = Instant::now();
        let mut budget = VerificationBudget::new(now);
        assert_eq!(
            budget.admit(u64::MAX, now).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let later = now + Duration::from_hours(1);
        budget
            .admit(BURST_BYTES, later)
            .expect("one full burst after idle");
        assert!(budget.admit(1, later).is_err());
        assert!(
            budget.admit(1, now).is_err(),
            "an older clock cannot grant credit"
        );
        assert!(
            budget.admit(1, later).is_err(),
            "nor can returning to the later clock"
        );
    }
}
