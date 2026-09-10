#![forbid(unsafe_code)]

//! The timeout guard — a technology of `xmip-core-resilience` (ADR-0048).
//!
//! An attempt that took longer than its limit is refused, even one that
//! succeeded: a late answer may be no answer to the caller, and the caller
//! decides what a late success means. A lenient guard judges failures only
//! and lets a late success stand. The guard cannot cut an attempt short — the
//! platform runs the operation, and a guard sees it only once it is over.

use std::time::Duration;

use resilience::{Attempt, Decision, Guard, TimeoutPolicy};

/// The timeout guard: an attempt may take this long.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeout {
    limit: Duration,
    lenient: bool,
}

impl Timeout {
    /// Refuse any attempt that took longer than `limit`.
    #[must_use]
    pub const fn new(limit: Duration) -> Self {
        Self {
            limit,
            lenient: false,
        }
    }

    /// The guard the platform's policy declares.
    #[must_use]
    pub const fn from_policy(policy: &TimeoutPolicy) -> Self {
        Self::new(policy.timeout)
    }

    /// Judge failures only: a success stands however long it took.
    #[must_use]
    pub const fn lenient(mut self) -> Self {
        self.lenient = true;
        self
    }

    /// How long an attempt may take.
    #[must_use]
    pub const fn limit(&self) -> Duration {
        self.limit
    }

    /// Whether a late success is refused.
    #[must_use]
    pub const fn is_lenient(&self) -> bool {
        self.lenient
    }
}

impl Guard for Timeout {
    fn technology(&self) -> &'static str {
        "timeout"
    }

    fn before(&self, _: u32) -> Decision {
        Decision::Proceed
    }

    fn after(&self, attempt: &Attempt) -> Decision {
        let judged = !self.lenient || !attempt.succeeded();
        if judged && attempt.elapsed > self.limit {
            Decision::Refuse(format!(
                "took {}ms, limit {}ms",
                attempt.elapsed.as_millis(),
                self.limit.as_millis()
            ))
        } else {
            Decision::Proceed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use resilience::{Failure, Guarded, execute};
    use std::cell::Cell;

    fn attempt(elapsed: Duration, failure: Option<Failure>) -> Attempt {
        Attempt {
            number: 1,
            elapsed,
            failure,
        }
    }

    #[test]
    fn a_late_success_is_refused_with_how_long_it_took_and_the_limit() {
        let timeout = Timeout::new(Duration::from_millis(10));
        assert_eq!(timeout.technology(), "timeout");
        assert_eq!(timeout.before(1), Decision::Proceed);
        assert_eq!(
            timeout.after(&attempt(Duration::from_millis(25), None)),
            Decision::Refuse("took 25ms, limit 10ms".into())
        );
        assert_eq!(
            timeout.after(&attempt(Duration::from_millis(10), None)),
            Decision::Proceed,
            "exactly the limit is within it"
        );
    }

    #[test]
    fn a_quick_attempt_stands_whether_it_succeeded_or_failed() {
        let timeout = Timeout::from_policy(&TimeoutPolicy {
            timeout: Duration::from_millis(10),
        });
        assert_eq!(timeout.limit(), Duration::from_millis(10));
        assert_eq!(
            timeout.after(&attempt(Duration::from_millis(1), None)),
            Decision::Proceed
        );
        assert_eq!(
            timeout.after(&attempt(
                Duration::from_millis(1),
                Some(Failure::retryable("again"))
            )),
            Decision::Proceed
        );
    }

    #[test]
    fn a_lenient_guard_lets_a_late_success_stand_but_refuses_a_late_failure() {
        let lenient = Timeout::new(Duration::from_millis(10)).lenient();
        assert!(lenient.is_lenient());
        assert!(!Timeout::new(Duration::ZERO).is_lenient());
        assert_eq!(
            lenient.after(&attempt(Duration::from_millis(25), None)),
            Decision::Proceed
        );
        assert_eq!(
            lenient.after(&attempt(
                Duration::from_millis(25),
                Some(Failure::permanent("broken"))
            )),
            Decision::Refuse("took 25ms, limit 10ms".into())
        );
    }

    #[test]
    fn under_execute_an_operation_that_overran_is_refused_and_a_quick_one_is_done() {
        let timeout = Timeout::new(Duration::ZERO);
        let guards: [&dyn Guard; 1] = [&timeout];
        let calls = Cell::new(0);
        let refused = execute(&guards, || {
            calls.set(calls.get() + 1);
            std::thread::sleep(Duration::from_millis(2));
            Ok("late")
        });
        assert_eq!(calls.get(), 1);
        match refused {
            Ok(Guarded::Refused(reason)) => assert!(reason.starts_with("took "), "{reason}"),
            other => panic!("expected a refusal, got {other:?}"),
        }

        let generous = Timeout::new(Duration::from_secs(60));
        let guards: [&dyn Guard; 1] = [&generous];
        assert_eq!(execute(&guards, || Ok("quick")), Ok(Guarded::Done("quick")));
    }

    /// Tries again on a retryable failure, as the retry technology does.
    struct Again(u32);

    impl Guard for Again {
        fn technology(&self) -> &'static str {
            "retry"
        }

        fn before(&self, _: u32) -> Decision {
            Decision::Proceed
        }

        fn after(&self, attempt: &Attempt) -> Decision {
            match &attempt.failure {
                Some(failure) if failure.is_retryable() && attempt.number < self.0 => {
                    Decision::Wait(Duration::ZERO)
                }
                _ => Decision::Proceed,
            }
        }
    }

    #[test]
    fn timeout_ahead_of_retry_refuses_a_slow_failure_before_retry_can_try_again() {
        let timeout = Timeout::new(Duration::ZERO);
        let guards: [&dyn Guard; 2] = [&timeout, &Again(3)];
        let calls = Cell::new(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            std::thread::sleep(Duration::from_millis(2));
            Err(Failure::retryable("again"))
        });
        assert!(matches!(outcome, Ok(Guarded::Refused(_))), "{outcome:?}");
        assert_eq!(calls.get(), 1, "the timeout decided before retry was asked");

        let guards: [&dyn Guard; 2] = [&Again(3), &timeout];
        calls.set(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            std::thread::sleep(Duration::from_millis(2));
            Err(Failure::retryable("again"))
        });
        assert!(matches!(outcome, Ok(Guarded::Refused(_))), "{outcome:?}");
        assert_eq!(
            calls.get(),
            3,
            "retry first: three attempts, then the timeout"
        );
    }
}
