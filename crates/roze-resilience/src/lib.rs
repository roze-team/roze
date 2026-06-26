use std::time::{Duration, Instant};

use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitSnapshot {
    pub tokens: f64,
    pub last_refill: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerSnapshot {
    pub failures: u32,
    pub open_until: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct RateLimitRegistry {
    states: DashMap<String, RateLimitState>,
}

#[derive(Debug, Clone, Copy)]
struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&self, key: impl Into<String>, config: RateLimitConfig) -> bool {
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(|| RateLimitState {
                tokens: config.burst as f64,
                last_refill: Instant::now(),
            });
        refill_tokens(&mut state, config);
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn snapshot(&self, key: &str) -> Option<RateLimitSnapshot> {
        self.states.get(key).map(|state| RateLimitSnapshot {
            tokens: state.tokens,
            last_refill: state.last_refill,
        })
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct BreakerRegistry {
    states: DashMap<String, BreakerState>,
}

#[derive(Debug, Clone, Copy)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

impl BreakerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self, key: impl Into<String>) -> bool {
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(|| BreakerState {
                failures: 0,
                open_until: None,
            });
        breaker_is_open(&mut state)
    }

    pub fn record_success(&self, key: impl Into<String>) {
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(|| BreakerState {
                failures: 0,
                open_until: None,
            });
        state.failures = 0;
        state.open_until = None;
    }

    pub fn record_failure(&self, key: impl Into<String>, config: BreakerConfig) {
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(|| BreakerState {
                failures: 0,
                open_until: None,
            });
        state.failures = state.failures.saturating_add(1);
        if state.failures >= config.failure_threshold.max(1) {
            state.failures = 0;
            state.open_until = Some(Instant::now() + config.reset_timeout);
        }
    }

    pub fn snapshot(&self, key: &str) -> Option<BreakerSnapshot> {
        self.states.get(key).map(|state| BreakerSnapshot {
            failures: state.failures,
            open_until: state.open_until,
        })
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

fn refill_tokens(state: &mut RateLimitState, config: RateLimitConfig) {
    let refill_secs = config.refill.as_secs_f64();
    if refill_secs <= 0.0 {
        state.tokens = config.burst as f64;
        state.last_refill = Instant::now();
        return;
    }

    let now = Instant::now();
    let elapsed = now.duration_since(state.last_refill).as_secs_f64();
    let tokens_to_add = elapsed / refill_secs;
    if tokens_to_add > 0.0 {
        state.tokens = (state.tokens + tokens_to_add).min(config.burst as f64);
        state.last_refill = now;
    }
}

fn breaker_is_open(state: &mut BreakerState) -> bool {
    if let Some(open_until) = state.open_until {
        if Instant::now() < open_until {
            return true;
        }
        state.open_until = None;
        state.failures = 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_consumes_and_refills_tokens() {
        let registry = RateLimitRegistry::new();
        let key = "svc:GET:/ready";
        let config = RateLimitConfig {
            burst: 1,
            refill: Duration::from_millis(10),
        };

        assert!(registry.allow(key, config));
        assert!(!registry.allow(key, config));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn breaker_opens_and_resets_by_key() {
        let registry = BreakerRegistry::new();
        let key = "svc:GetUser";
        let config = BreakerConfig {
            failure_threshold: 2,
            reset_timeout: Duration::from_millis(1),
        };

        assert!(!registry.is_open(key));
        registry.record_failure(key, config);
        assert!(!registry.is_open(key));
        registry.record_failure(key, config);
        assert!(registry.is_open(key));

        std::thread::sleep(Duration::from_millis(2));
        assert!(!registry.is_open(key));
    }

    #[test]
    fn breaker_success_closes_state() {
        let registry = BreakerRegistry::new();
        let key = "svc:GetUser";
        let config = BreakerConfig {
            failure_threshold: 1,
            reset_timeout: Duration::from_secs(1),
        };

        registry.record_failure(key, config);
        assert!(registry.is_open(key));
        registry.record_success(key);
        assert!(!registry.is_open(key));
    }
}
