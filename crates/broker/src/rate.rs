//! Sliding-window enforcement of policy rate limits.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, PoisonError},
    time::Instant,
};

use aidex_policy::RateLimit;

/// Remembers recent executions per key (session, tool, rule).
#[derive(Debug, Default)]
pub(crate) struct RateLimiter {
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// Record one execution under `key` if `limit` allows it; `false` when the limit
    /// is already reached within its window.
    pub fn try_acquire(&self, key: &str, limit: RateLimit) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        let window = windows.entry(key.to_owned()).or_default();
        while window
            .front()
            .is_some_and(|started| now.duration_since(*started) >= limit.per)
        {
            window.pop_front();
        }
        if window.len() >= limit.max as usize {
            return false;
        }
        window.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn limits_apply_per_key_within_the_window() {
        let limiter = RateLimiter::default();
        let limit = RateLimit {
            max: 2,
            per: Duration::from_secs(60),
        };
        assert!(limiter.try_acquire("a", limit));
        assert!(limiter.try_acquire("a", limit));
        assert!(!limiter.try_acquire("a", limit));
        assert!(limiter.try_acquire("b", limit));
    }
}
