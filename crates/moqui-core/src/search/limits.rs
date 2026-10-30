//! Request-scoped search budgets; nested backend calls share the same deadline.
use moqui_lib::error::Error;
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
pub struct QueryLimits {
    pub timeout: Duration,
    pub buckets: usize,
}
impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            buckets: 10_000,
        }
    }
}
#[derive(Clone, Copy)]
struct Active {
    deadline: Instant,
    buckets: usize,
}
thread_local! {static ACTIVE:RefCell<Option<Active>>=const{RefCell::new(None)};}
struct Restore(Option<Active>);
impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.0);
    }
}
pub fn with_limits<T>(limits: QueryLimits, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let next = Active {
        deadline: Instant::now() + limits.timeout,
        buckets: limits.buckets.clamp(1, 65_536),
    };
    let old = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let old = *active;
        *active = Some(match old {
            Some(old) => Active {
                deadline: old.deadline.min(next.deadline),
                buckets: old.buckets.min(next.buckets),
            },
            None => next,
        });
        old
    });
    let _restore = Restore(old);
    body()
}
pub fn remaining() -> Result<Option<Duration>, Error> {
    ACTIVE.with(|active| match *active.borrow() {
        None => Ok(None),
        Some(active) => {
            let remaining = active.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                Err(Error::new("report query timeout exceeded"))
            } else {
                Ok(Some(remaining))
            }
        }
    })
}
pub(crate) fn buckets() -> usize {
    ACTIVE.with(|active| active.borrow().map_or(65_536, |a| a.buckets))
}

/// The source of an error for a result over the bucket budget. A report reads it as an incomplete
/// result, not a failed query, without matching the message text.
#[derive(Debug)]
pub struct TooManyGroups;
impl std::fmt::Display for TooManyGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the report bucket limit was reached")
    }
}
impl std::error::Error for TooManyGroups {}

#[track_caller]
pub(crate) fn too_many_groups(message: impl Into<String>) -> Error {
    Error::new(message).with_source(TooManyGroups)
}

/// Whether an error, or one it carries, is over the bucket budget.
pub fn is_too_many_groups(error: &Error) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(error) = source {
        if error.is::<TooManyGroups>() {
            return true;
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_limits_restore_without_leaking() {
        assert_eq!(buckets(), 65_536);
        with_limits(
            QueryLimits {
                buckets: 5,
                ..Default::default()
            },
            || {
                assert_eq!(buckets(), 5);
                with_limits(
                    QueryLimits {
                        buckets: 2,
                        ..Default::default()
                    },
                    || {
                        assert_eq!(buckets(), 2);
                        Ok(())
                    },
                )?;
                assert_eq!(buckets(), 5);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(buckets(), 65_536);
    }
    #[test]
    fn too_many_groups_is_known_by_its_source_not_its_text() {
        let error = too_many_groups("an aggregation level returned more than 5 groups");
        assert!(is_too_many_groups(&error));
        assert_eq!(error.message(), "an aggregation level returned more than 5 groups");
        assert!(is_too_many_groups(&Error::new("report failed").with_source(error)));
        assert!(!is_too_many_groups(&Error::new("too many groups")));
    }
}
