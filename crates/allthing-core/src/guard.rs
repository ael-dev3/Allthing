// SPDX-License-Identifier: AGPL-3.0-or-later
//! Budget primitives that keep the Worker inside Cloudflare's free-plan
//! limits and polite to upstreams: a token bucket, a circuit breaker with
//! exponential backoff, a per-invocation subrequest budget, and cache
//! freshness classification. Pure logic with injected clocks, so it is
//! tested natively.

/// Refills continuously; each upstream request takes one token.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_ms: f64,
    tokens: f64,
    updated_ms: f64,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_per_second: f64, now_ms: f64) -> TokenBucket {
        TokenBucket {
            capacity: f64::from(capacity),
            refill_per_ms: refill_per_second / 1000.0,
            tokens: f64::from(capacity),
            updated_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: f64) {
        let elapsed = (now_ms - self.updated_ms).max(0.0);
        self.tokens = (self.tokens + elapsed * self.refill_per_ms).min(self.capacity);
        self.updated_ms = now_ms;
    }

    pub fn try_take(&mut self, now_ms: f64) -> bool {
        self.refill(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Milliseconds until a token is available.
    pub fn wait_ms(&mut self, now_ms: f64) -> f64 {
        self.refill(now_ms);
        if self.tokens >= 1.0 { 0.0 } else { (1.0 - self.tokens) / self.refill_per_ms }
    }

    /// Whole tokens available now, without taking any.
    pub fn available(&mut self, now_ms: f64) -> u32 {
        self.refill(now_ms);
        self.tokens.floor() as u32
    }
}

/// Opens after consecutive failures and stays open for an exponentially
/// growing backoff (or an upstream's `Retry-After`), so a struggling upstream
/// is left alone while cached copies are served.
#[derive(Clone, Debug)]
pub struct Breaker {
    failures: u32,
    open_until_ms: f64,
    threshold: u32,
    base_ms: f64,
    max_ms: f64,
}

impl Breaker {
    /// Opens after `threshold` consecutive failures; backoff starts at
    /// `base_ms` and doubles up to `max_ms`.
    pub fn new(threshold: u32, base_ms: f64, max_ms: f64) -> Breaker {
        Breaker { failures: 0, open_until_ms: 0.0, threshold: threshold.max(1), base_ms, max_ms }
    }

    pub fn allow(&self, now_ms: f64) -> bool {
        now_ms >= self.open_until_ms
    }

    /// Milliseconds until requests are allowed again.
    pub fn retry_after_ms(&self, now_ms: f64) -> f64 {
        (self.open_until_ms - now_ms).max(0.0)
    }

    pub fn success(&mut self) {
        self.failures = 0;
        self.open_until_ms = 0.0;
    }

    /// Record a failure. `retry_after_ms` (from a 429/503) takes precedence
    /// over the computed backoff when it is longer.
    pub fn failure(&mut self, now_ms: f64, retry_after_ms: Option<f64>) {
        self.failures = self.failures.saturating_add(1);
        let rate_limited = retry_after_ms.is_some();
        if self.failures >= self.threshold || rate_limited {
            let exponent = self.failures.saturating_sub(self.threshold).min(16);
            let backoff = (self.base_ms * f64::from(1u32 << exponent)).min(self.max_ms);
            let wait = retry_after_ms.map_or(backoff, |r| r.max(backoff).min(self.max_ms));
            self.open_until_ms = now_ms + wait;
        }
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// Cloudflare caps subrequests (fetch and Cache API calls together) per
/// invocation. Every outgoing call takes from this budget first.
#[derive(Clone, Debug)]
pub struct SubrequestBudget {
    limit: u32,
    used: u32,
}

impl SubrequestBudget {
    pub fn new(limit: u32) -> SubrequestBudget {
        SubrequestBudget { limit, used: 0 }
    }

    pub fn try_take(&mut self, n: u32) -> bool {
        if self.used + n <= self.limit {
            self.used += n;
            true
        } else {
            false
        }
    }

    pub fn remaining(&self) -> u32 {
        self.limit - self.used
    }

    pub fn used(&self) -> u32 {
        self.used
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Serve as is.
    Fresh,
    /// Serve, and refresh in the background.
    Stale,
    /// Too old to serve unless the upstream is down.
    Expired,
}

pub fn freshness(age_ms: f64, fresh_ms: f64, stale_ms: f64) -> Freshness {
    if age_ms < fresh_ms {
        Freshness::Fresh
    } else if age_ms < stale_ms {
        Freshness::Stale
    } else {
        Freshness::Expired
    }
}

/// Parse a `Retry-After` header given in seconds (HTTP dates are ignored).
pub fn parse_retry_after(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).map(|s| s * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_limits_bursts_and_refills() {
        let mut bucket = TokenBucket::new(3, 2.0, 0.0);
        assert!(bucket.try_take(0.0));
        assert!(bucket.try_take(0.0));
        assert!(bucket.try_take(0.0));
        assert!(!bucket.try_take(0.0));
        assert!((bucket.wait_ms(0.0) - 500.0).abs() < 1e-9);
        assert!(bucket.try_take(500.0));
        assert!(!bucket.try_take(600.0));
        // Never refills past capacity.
        assert!(bucket.try_take(100_000.0) && bucket.try_take(100_000.0) && bucket.try_take(100_000.0));
        assert!(!bucket.try_take(100_000.0));
    }

    #[test]
    fn bucket_reports_whole_tokens_without_taking_them() {
        let mut bucket = TokenBucket::new(40, 10.0, 0.0);
        assert_eq!(bucket.available(0.0), 40);
        assert_eq!(bucket.available(0.0), 40, "looking takes nothing");
        for _ in 0..16 {
            assert!(bucket.try_take(0.0));
        }
        assert_eq!(bucket.available(0.0), 24);
        // 10 a second: half a token after 50 ms is not a token yet.
        assert_eq!(bucket.available(50.0), 24);
        assert_eq!(bucket.available(1000.0), 34);
        assert_eq!(bucket.available(60_000.0), 40);
    }

    #[test]
    fn breaker_backs_off_exponentially() {
        let mut breaker = Breaker::new(2, 1000.0, 8000.0);
        breaker.failure(0.0, None);
        assert!(breaker.allow(0.0), "one failure is tolerated");
        breaker.failure(0.0, None);
        assert!(!breaker.allow(999.0));
        assert!(breaker.allow(1000.0));
        breaker.failure(1000.0, None);
        assert_eq!(breaker.retry_after_ms(1000.0), 2000.0);
        breaker.failure(3000.0, None);
        assert_eq!(breaker.retry_after_ms(3000.0), 4000.0);
        for _ in 0..10 {
            breaker.failure(10_000.0, None);
        }
        assert_eq!(breaker.retry_after_ms(10_000.0), 8000.0, "capped");
        breaker.success();
        assert!(breaker.allow(10_000.0));
        assert_eq!(breaker.failures(), 0);
    }

    #[test]
    fn breaker_honours_retry_after() {
        let mut breaker = Breaker::new(3, 1000.0, 60_000.0);
        breaker.failure(0.0, Some(30_000.0));
        assert!(!breaker.allow(29_999.0));
        assert!(breaker.allow(30_000.0));
        let mut breaker = Breaker::new(3, 1000.0, 60_000.0);
        breaker.failure(0.0, Some(3_600_000.0));
        assert_eq!(breaker.retry_after_ms(0.0), 60_000.0, "capped at max");
    }

    #[test]
    fn subrequest_budget() {
        let mut budget = SubrequestBudget::new(5);
        assert!(budget.try_take(3));
        assert!(!budget.try_take(3));
        assert!(budget.try_take(2));
        assert_eq!(budget.remaining(), 0);
        assert_eq!(budget.used(), 5);
    }

    #[test]
    fn freshness_and_retry_after() {
        assert_eq!(freshness(10.0, 60.0, 600.0), Freshness::Fresh);
        assert_eq!(freshness(60.0, 60.0, 600.0), Freshness::Stale);
        assert_eq!(freshness(600.0, 60.0, 600.0), Freshness::Expired);
        assert_eq!(parse_retry_after("30"), Some(30_000.0));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
    }
}
