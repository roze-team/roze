use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheddingConfig {
    pub concurrency: usize,
    pub window: Duration,
    pub min_samples: u64,
    pub max_avg_latency: Duration,
    pub max_failure_ratio_per_mille: u32,
    pub cool_down: Duration,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SheddingSnapshot {
    pub in_flight: usize,
    pub samples: usize,
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

#[derive(Debug, Default)]
pub struct SheddingRegistry {
    states: DashMap<String, SheddingState>,
}

#[derive(Debug, Clone, Copy)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

#[derive(Debug)]
struct SheddingState {
    in_flight: usize,
    samples: VecDeque<SheddingSample>,
    open_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
struct SheddingSample {
    at: Instant,
    latency: Duration,
    success: bool,
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

impl SheddingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&self, key: impl Into<String>, config: SheddingConfig) -> bool {
        let now = Instant::now();
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(SheddingState::default);
        prune_shedding_samples(&mut state, now, config.window);
        if shedding_is_open(&mut state, now) {
            return false;
        }
        if state.in_flight >= config.concurrency.max(1) {
            state.open_until = Some(now + config.cool_down);
            return false;
        }
        state.in_flight = state.in_flight.saturating_add(1);
        true
    }

    pub fn record(
        &self,
        key: impl Into<String>,
        success: bool,
        latency: Duration,
        config: SheddingConfig,
    ) {
        let now = Instant::now();
        let mut state = self
            .states
            .entry(key.into())
            .or_insert_with(SheddingState::default);
        state.in_flight = state.in_flight.saturating_sub(1);
        state.samples.push_back(SheddingSample {
            at: now,
            latency,
            success,
        });
        prune_shedding_samples(&mut state, now, config.window);
        if should_shed(&state, config) {
            state.open_until = Some(now + config.cool_down);
        }
    }

    pub fn snapshot(&self, key: &str) -> Option<SheddingSnapshot> {
        self.states.get(key).map(|state| SheddingSnapshot {
            in_flight: state.in_flight,
            samples: state.samples.len(),
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

impl Default for SheddingState {
    fn default() -> Self {
        Self {
            in_flight: 0,
            samples: VecDeque::new(),
            open_until: None,
        }
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

fn shedding_is_open(state: &mut SheddingState, now: Instant) -> bool {
    if let Some(open_until) = state.open_until {
        if now < open_until {
            return true;
        }
        state.open_until = None;
    }
    false
}

fn prune_shedding_samples(state: &mut SheddingState, now: Instant, window: Duration) {
    while state
        .samples
        .front()
        .is_some_and(|sample| now.duration_since(sample.at) > window)
    {
        state.samples.pop_front();
    }
}

fn should_shed(state: &SheddingState, config: SheddingConfig) -> bool {
    let sample_count = state.samples.len() as u64;
    if sample_count < config.min_samples.max(1) {
        return false;
    }
    let total_latency: Duration = state.samples.iter().map(|sample| sample.latency).sum();
    let avg_latency = total_latency / state.samples.len() as u32;
    if avg_latency > config.max_avg_latency {
        return true;
    }
    let failures = state
        .samples
        .iter()
        .filter(|sample| !sample.success)
        .count() as u64;
    let failure_ratio = failures.saturating_mul(1_000) / sample_count.max(1);
    failure_ratio > u64::from(config.max_failure_ratio_per_mille)
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

    #[test]
    fn shedding_rejects_when_concurrency_is_full() {
        let registry = SheddingRegistry::new();
        let key = "svc:GET:/busy";
        let config = SheddingConfig {
            concurrency: 1,
            window: Duration::from_secs(1),
            min_samples: 10,
            max_avg_latency: Duration::from_secs(1),
            max_failure_ratio_per_mille: 500,
            cool_down: Duration::from_secs(1),
        };

        assert!(registry.allow(key, config));
        assert!(!registry.allow(key, config));
        let snapshot = registry.snapshot(key).expect("snapshot");
        assert_eq!(snapshot.in_flight, 1);
        assert!(snapshot.open_until.is_some());
    }

    #[test]
    fn shedding_opens_after_failure_ratio_crosses_threshold() {
        let registry = SheddingRegistry::new();
        let key = "svc:GetUser";
        let config = SheddingConfig {
            concurrency: 10,
            window: Duration::from_secs(1),
            min_samples: 2,
            max_avg_latency: Duration::from_secs(1),
            max_failure_ratio_per_mille: 499,
            cool_down: Duration::from_secs(1),
        };

        assert!(registry.allow(key, config));
        registry.record(key, false, Duration::from_millis(10), config);
        assert!(registry.allow(key, config));
        registry.record(key, true, Duration::from_millis(10), config);

        assert!(!registry.allow(key, config));
    }
}
