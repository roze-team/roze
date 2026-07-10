use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use dashmap::DashMap;

const SHEDDING_BUCKETS: usize = 50;
static RETRY_JITTER_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

pub fn exponential_backoff_cap(base: Duration, max: Duration, attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(31) as u32;
    base.saturating_mul(1_u32 << exponent).min(max)
}

pub fn full_jitter_delay(base: Duration, max: Duration, attempt: usize) -> Duration {
    full_jitter_delay_with_sample(base, max, attempt, next_retry_jitter())
}

fn full_jitter_delay_with_sample(
    base: Duration,
    max: Duration,
    attempt: usize,
    sample: u64,
) -> Duration {
    let cap = exponential_backoff_cap(base, max, attempt);
    let cap_nanos = cap.as_nanos().min(u128::from(u64::MAX)) as u64;
    let jitter_nanos = if cap_nanos == u64::MAX {
        sample
    } else {
        sample % (cap_nanos + 1)
    };
    Duration::from_nanos(jitter_nanos)
}

fn next_retry_jitter() -> u64 {
    let mut current = RETRY_JITTER_STATE.load(Ordering::Relaxed);
    loop {
        let mut next = current;
        next ^= next << 13;
        next ^= next >> 7;
        next ^= next << 17;
        match RETRY_JITTER_STATE.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

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
pub enum BreakerPhase {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerPermit {
    Closed,
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerDecision {
    Allow(BreakerPermit),
    Reject,
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
    pub phase: BreakerPhase,
    pub open_until: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SheddingSnapshot {
    pub in_flight: usize,
    pub samples: u64,
    pub buckets: usize,
    pub open_until: Option<Instant>,
}

#[derive(Debug)]
pub struct RetryBudgetRegistry {
    states: DashMap<String, RetryBudgetState>,
    window: Duration,
}

#[derive(Debug, Clone, Copy)]
struct RetryBudgetState {
    window_start: Instant,
    calls: u64,
    retries: u64,
}

impl Default for RetryBudgetRegistry {
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

impl RetryBudgetRegistry {
    pub fn new(window: Duration) -> Self {
        Self {
            states: DashMap::new(),
            window,
        }
    }

    pub fn record_call(&self, key: &str) {
        let now = Instant::now();
        let mut state = self
            .states
            .entry(key.to_string())
            .or_insert_with(|| RetryBudgetState {
                window_start: now,
                calls: 0,
                retries: 0,
            });
        reset_retry_budget_window(&mut state, now, self.window);
        state.calls = state.calls.saturating_add(1);
    }

    pub fn allow_retry(&self, key: &str, budget_percent: Option<u32>) -> bool {
        let Some(budget_percent) = budget_percent else {
            return true;
        };
        let now = Instant::now();
        let mut state = self
            .states
            .entry(key.to_string())
            .or_insert_with(|| RetryBudgetState {
                window_start: now,
                calls: 1,
                retries: 0,
            });
        reset_retry_budget_window(&mut state, now, self.window);
        let retry_budget = state
            .calls
            .saturating_mul(u64::from(budget_percent.min(100)))
            / 100;
        let allowed = state.retries < retry_budget.max(1);
        if allowed {
            state.retries = state.retries.saturating_add(1);
        }
        allowed
    }
}

fn reset_retry_budget_window(state: &mut RetryBudgetState, now: Instant, window: Duration) {
    if now.duration_since(state.window_start) > window {
        state.window_start = now;
        state.calls = 0;
        state.retries = 0;
    }
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
    phase: BreakerPhase,
    open_until: Option<Instant>,
}

impl Default for BreakerState {
    fn default() -> Self {
        Self {
            failures: 0,
            phase: BreakerPhase::Closed,
            open_until: None,
        }
    }
}

#[derive(Debug, Default)]
struct SheddingState {
    in_flight: usize,
    buckets: VecDeque<SheddingBucket>,
    open_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
struct SheddingBucket {
    started_at: Instant,
    samples: u64,
    failures: u64,
    total_latency: Duration,
}

impl BreakerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&self, key: impl Into<String>) -> BreakerDecision {
        let mut state = self.states.entry(key.into()).or_default();
        match state.phase {
            BreakerPhase::Closed => BreakerDecision::Allow(BreakerPermit::Closed),
            BreakerPhase::Open => {
                if state.open_until.is_some_and(|until| Instant::now() < until) {
                    BreakerDecision::Reject
                } else {
                    state.phase = BreakerPhase::HalfOpen;
                    state.open_until = None;
                    BreakerDecision::Allow(BreakerPermit::Probe)
                }
            }
            BreakerPhase::HalfOpen => BreakerDecision::Reject,
        }
    }

    pub fn record_success(&self, key: impl Into<String>, permit: BreakerPermit) {
        let mut state = self.states.entry(key.into()).or_default();
        match permit {
            BreakerPermit::Closed if state.phase == BreakerPhase::Closed => {
                state.failures = 0;
            }
            BreakerPermit::Probe if state.phase == BreakerPhase::HalfOpen => {
                state.failures = 0;
                state.phase = BreakerPhase::Closed;
                state.open_until = None;
            }
            BreakerPermit::Closed | BreakerPermit::Probe => {}
        }
    }

    pub fn record_failure(
        &self,
        key: impl Into<String>,
        permit: BreakerPermit,
        config: BreakerConfig,
    ) {
        let mut state = self.states.entry(key.into()).or_default();
        match permit {
            BreakerPermit::Closed if state.phase == BreakerPhase::Closed => {
                state.failures = state.failures.saturating_add(1);
                if state.failures >= config.failure_threshold.max(1) {
                    open_breaker(&mut state, config.reset_timeout);
                }
            }
            BreakerPermit::Probe if state.phase == BreakerPhase::HalfOpen => {
                open_breaker(&mut state, config.reset_timeout);
            }
            BreakerPermit::Closed | BreakerPermit::Probe => {}
        }
    }

    pub fn cancel(&self, key: &str, permit: BreakerPermit, config: BreakerConfig) {
        if permit != BreakerPermit::Probe {
            return;
        }
        if let Some(mut state) = self.states.get_mut(key) {
            if state.phase == BreakerPhase::HalfOpen {
                open_breaker(&mut state, config.reset_timeout);
            }
        }
    }

    pub fn snapshot(&self, key: &str) -> Option<BreakerSnapshot> {
        self.states.get(key).map(|state| BreakerSnapshot {
            failures: state.failures,
            phase: state.phase,
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
        let mut state = self.states.entry(key.into()).or_default();
        prune_shedding_buckets(&mut state, now, config.window);
        if shedding_is_open(&mut state, now) {
            return false;
        }
        if state.in_flight >= config.concurrency.max(1) {
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
        let mut state = self.states.entry(key.into()).or_default();
        state.in_flight = state.in_flight.saturating_sub(1);
        record_shedding_sample(&mut state, now, success, latency, config.window);
        if should_shed(&state, config) {
            state.open_until = Some(now + config.cool_down);
        }
    }

    pub fn release(&self, key: &str) {
        if let Some(mut state) = self.states.get_mut(key) {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
    }

    pub fn snapshot(&self, key: &str) -> Option<SheddingSnapshot> {
        self.states.get(key).map(|state| SheddingSnapshot {
            in_flight: state.in_flight,
            samples: state.buckets.iter().map(|bucket| bucket.samples).sum(),
            buckets: state.buckets.len(),
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

fn open_breaker(state: &mut BreakerState, reset_timeout: Duration) {
    state.failures = 0;
    state.phase = BreakerPhase::Open;
    state.open_until = Some(Instant::now() + reset_timeout);
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

fn shedding_bucket_duration(window: Duration) -> Duration {
    (window / SHEDDING_BUCKETS as u32).max(Duration::from_millis(1))
}

fn prune_shedding_buckets(state: &mut SheddingState, now: Instant, window: Duration) {
    let effective_window = window.max(shedding_bucket_duration(window));
    while state
        .buckets
        .front()
        .is_some_and(|bucket| now.duration_since(bucket.started_at) >= effective_window)
    {
        state.buckets.pop_front();
    }
}

fn record_shedding_sample(
    state: &mut SheddingState,
    now: Instant,
    success: bool,
    latency: Duration,
    window: Duration,
) {
    prune_shedding_buckets(state, now, window);
    let bucket_duration = shedding_bucket_duration(window);
    if let Some(bucket) = state
        .buckets
        .back_mut()
        .filter(|bucket| now.duration_since(bucket.started_at) < bucket_duration)
    {
        bucket.samples = bucket.samples.saturating_add(1);
        bucket.failures = bucket.failures.saturating_add(u64::from(!success));
        bucket.total_latency = bucket.total_latency.saturating_add(latency);
        return;
    }

    state.buckets.push_back(SheddingBucket {
        started_at: now,
        samples: 1,
        failures: u64::from(!success),
        total_latency: latency,
    });
    while state.buckets.len() > SHEDDING_BUCKETS {
        state.buckets.pop_front();
    }
}

fn should_shed(state: &SheddingState, config: SheddingConfig) -> bool {
    let sample_count = state
        .buckets
        .iter()
        .map(|bucket| bucket.samples)
        .sum::<u64>();
    if sample_count < config.min_samples.max(1) {
        return false;
    }
    let total_latency = state.buckets.iter().fold(Duration::ZERO, |total, bucket| {
        total.saturating_add(bucket.total_latency)
    });
    if total_latency.as_nanos() / u128::from(sample_count) > config.max_avg_latency.as_nanos() {
        return true;
    }
    let failures = state
        .buckets
        .iter()
        .map(|bucket| bucket.failures)
        .sum::<u64>();
    let failure_ratio = failures.saturating_mul(1_000) / sample_count.max(1);
    failure_ratio > u64::from(config.max_failure_ratio_per_mille)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed_permit(decision: BreakerDecision) -> BreakerPermit {
        match decision {
            BreakerDecision::Allow(permit) => permit,
            BreakerDecision::Reject => panic!("breaker unexpectedly rejected request"),
        }
    }

    #[test]
    fn exponential_backoff_cap_doubles_and_saturates() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);

        assert_eq!(
            exponential_backoff_cap(base, max, 1),
            Duration::from_millis(100)
        );
        assert_eq!(
            exponential_backoff_cap(base, max, 2),
            Duration::from_millis(200)
        );
        assert_eq!(
            exponential_backoff_cap(base, max, 3),
            Duration::from_millis(400)
        );
        assert_eq!(exponential_backoff_cap(base, max, 4), max);
        assert_eq!(exponential_backoff_cap(base, max, usize::MAX), max);
    }

    #[test]
    fn full_jitter_delay_stays_inside_exponential_cap() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);

        assert_eq!(
            full_jitter_delay_with_sample(base, max, 2, 0),
            Duration::ZERO
        );
        assert_eq!(
            full_jitter_delay_with_sample(base, max, 2, 200_000_000),
            Duration::from_millis(200)
        );
        assert!(full_jitter_delay(base, max, 3) <= Duration::from_millis(400));
        assert_eq!(full_jitter_delay(Duration::ZERO, max, 1), Duration::ZERO);
    }

    #[test]
    fn retry_budget_is_bounded_per_key() {
        let registry = RetryBudgetRegistry::new(Duration::from_secs(60));
        registry.record_call("catalog:GetUser");
        assert!(registry.allow_retry("catalog:GetUser", Some(1)));
        assert!(!registry.allow_retry("catalog:GetUser", Some(1)));

        assert!(registry.allow_retry("orders:GetUser", None));
    }

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
    fn breaker_allows_exactly_one_half_open_probe() {
        let registry = BreakerRegistry::new();
        let key = "svc:GetUser";
        let config = BreakerConfig {
            failure_threshold: 2,
            reset_timeout: Duration::from_millis(1),
        };

        let first = allowed_permit(registry.allow(key));
        registry.record_failure(key, first, config);
        let second = allowed_permit(registry.allow(key));
        assert_eq!(second, BreakerPermit::Closed);
        registry.record_failure(key, second, config);
        assert_eq!(registry.allow(key), BreakerDecision::Reject);

        std::thread::sleep(Duration::from_millis(2));
        let probe = allowed_permit(registry.allow(key));
        assert_eq!(probe, BreakerPermit::Probe);
        assert_eq!(registry.allow(key), BreakerDecision::Reject);

        registry.record_success(key, probe);
        assert_eq!(
            registry.allow(key),
            BreakerDecision::Allow(BreakerPermit::Closed)
        );
    }

    #[test]
    fn stale_closed_success_cannot_close_newer_open_state() {
        let registry = BreakerRegistry::new();
        let key = "svc:GetUser";
        let config = BreakerConfig {
            failure_threshold: 1,
            reset_timeout: Duration::from_secs(1),
        };

        let failing = allowed_permit(registry.allow(key));
        let stale_success = allowed_permit(registry.allow(key));
        registry.record_failure(key, failing, config);
        registry.record_success(key, stale_success);

        let snapshot = registry.snapshot(key).expect("snapshot");
        assert_eq!(snapshot.phase, BreakerPhase::Open);
        assert_eq!(registry.allow(key), BreakerDecision::Reject);
    }

    #[test]
    fn cancelled_half_open_probe_reopens_without_failure_count() {
        let registry = BreakerRegistry::new();
        let key = "svc:GetUser";
        let config = BreakerConfig {
            failure_threshold: 1,
            reset_timeout: Duration::from_millis(1),
        };

        let permit = allowed_permit(registry.allow(key));
        registry.record_failure(key, permit, config);
        std::thread::sleep(Duration::from_millis(2));
        let probe = allowed_permit(registry.allow(key));
        assert_eq!(probe, BreakerPermit::Probe);

        registry.cancel(key, probe, config);
        let snapshot = registry.snapshot(key).expect("snapshot");
        assert_eq!(snapshot.phase, BreakerPhase::Open);
        assert_eq!(snapshot.failures, 0);
        assert_eq!(registry.allow(key), BreakerDecision::Reject);
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
        assert!(snapshot.open_until.is_none());

        registry.record(key, true, Duration::from_millis(10), config);
        assert!(registry.allow(key, config));
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

    #[test]
    fn shedding_release_returns_inflight_slot_without_recording_sample() {
        let registry = SheddingRegistry::new();
        let key = "svc:GET:/cancelled";
        let config = SheddingConfig {
            concurrency: 1,
            window: Duration::from_secs(1),
            min_samples: 1,
            max_avg_latency: Duration::from_secs(1),
            max_failure_ratio_per_mille: 0,
            cool_down: Duration::from_secs(1),
        };

        assert!(registry.allow(key, config));
        registry.release(key);

        let snapshot = registry.snapshot(key).expect("snapshot");
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.samples, 0);
        assert_eq!(snapshot.buckets, 0);
        assert!(registry.allow(key, config));
    }

    #[test]
    fn shedding_rolling_window_keeps_memory_bounded_by_bucket_count() {
        let mut state = SheddingState::default();
        let started_at = Instant::now();
        let window = Duration::from_millis(100);

        for index in 0..10_000 {
            record_shedding_sample(
                &mut state,
                started_at + Duration::from_millis(index),
                index % 10 != 0,
                Duration::from_millis(5),
                window,
            );
            assert!(state.buckets.len() <= SHEDDING_BUCKETS);
        }

        assert!(state.buckets.len() <= SHEDDING_BUCKETS);
        assert!(
            state
                .buckets
                .iter()
                .map(|bucket| bucket.samples)
                .sum::<u64>()
                <= 100
        );
    }
}
