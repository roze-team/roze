use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::registry::{instance_score, weighted_instances, ServiceInstance};

pub trait Balancer: Send + Sync + 'static {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalancerKind {
    FirstAvailable,
    RoundRobin,
    WeightedRoundRobin,
    PowerOfTwoChoices,
    HealthAware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Success,
    Failure,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
pub struct EwmaP2cConfig {
    pub decay: Duration,
    pub initial_latency: Duration,
    pub force_pick_after: Duration,
    pub stale_after: Duration,
}

impl Default for EwmaP2cConfig {
    fn default() -> Self {
        Self {
            decay: Duration::from_secs(10),
            initial_latency: Duration::from_secs(1),
            force_pick_after: Duration::from_secs(1),
            stale_after: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceLoadSnapshot {
    pub inflight: u64,
    pub ewma_latency: Duration,
    pub success_per_mille: u64,
}

#[derive(Debug)]
struct InstanceLoad {
    inflight: AtomicU64,
    ewma_latency_nanos: AtomicU64,
    success_per_mille: AtomicU64,
    last_seen_nanos: AtomicU64,
    last_updated_nanos: AtomicU64,
    last_picked_nanos: AtomicU64,
}

impl InstanceLoad {
    fn new(config: EwmaP2cConfig, now_nanos: u64) -> Self {
        Self {
            inflight: AtomicU64::new(0),
            ewma_latency_nanos: AtomicU64::new(duration_nanos(config.initial_latency)),
            success_per_mille: AtomicU64::new(1_000),
            last_seen_nanos: AtomicU64::new(now_nanos),
            last_updated_nanos: AtomicU64::new(0),
            last_picked_nanos: AtomicU64::new(now_nanos),
        }
    }

    fn load(&self) -> u128 {
        let latency = self.ewma_latency_nanos.load(Ordering::Relaxed).max(1);
        let inflight = self.inflight.load(Ordering::Relaxed).saturating_add(1);
        let success_penalty = 2_000u64
            .saturating_sub(self.success_per_mille.load(Ordering::Relaxed).min(1_000))
            .max(1_000);
        integer_sqrt(latency as u128)
            .saturating_mul(inflight as u128)
            .saturating_mul(success_penalty as u128)
    }

    fn decrement_inflight(&self) {
        let _ = self
            .inflight
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            });
    }

    fn snapshot(&self) -> InstanceLoadSnapshot {
        InstanceLoadSnapshot {
            inflight: self.inflight.load(Ordering::Relaxed),
            ewma_latency: Duration::from_nanos(self.ewma_latency_nanos.load(Ordering::Relaxed)),
            success_per_mille: self.success_per_mille.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EwmaP2cBalancer {
    config: EwmaP2cConfig,
    started: Instant,
    cursor: Arc<AtomicU64>,
    states: Arc<DashMap<String, Arc<InstanceLoad>>>,
}

impl Default for EwmaP2cBalancer {
    fn default() -> Self {
        Self::new(EwmaP2cConfig::default())
    }
}

impl EwmaP2cBalancer {
    pub fn new(config: EwmaP2cConfig) -> Self {
        Self {
            config,
            started: Instant::now(),
            cursor: Arc::new(AtomicU64::new(0)),
            states: Arc::new(DashMap::new()),
        }
    }

    #[must_use = "dropping the lease immediately records a cancelled attempt"]
    pub fn pick_tracked(&self, instances: &[ServiceInstance]) -> Option<AttemptLease> {
        let now_nanos = self.now_nanos();
        self.synchronize(instances, now_nanos);
        let instance = self.choose(instances, now_nanos)?.clone();
        let key = instance_identity(&instance);
        let state = self.state_for(&key, now_nanos);
        state.inflight.fetch_add(1, Ordering::AcqRel);
        state.last_picked_nanos.store(now_nanos, Ordering::Relaxed);
        Some(AttemptLease {
            instance,
            state,
            config: self.config,
            started: Instant::now(),
            clock_started: self.started,
            settled: false,
        })
    }

    pub fn synchronize(&self, instances: &[ServiceInstance], now_nanos: u64) {
        for instance in instances {
            let key = instance_identity(instance);
            self.state_for(&key, now_nanos)
                .last_seen_nanos
                .store(now_nanos, Ordering::Relaxed);
        }
        let stale_nanos = duration_nanos(self.config.stale_after);
        self.states.retain(|_, state| {
            now_nanos.saturating_sub(state.last_seen_nanos.load(Ordering::Relaxed)) <= stale_nanos
        });
    }

    pub fn prune(&self) {
        self.synchronize(&[], self.now_nanos());
    }

    pub fn state_len(&self) -> usize {
        self.states.len()
    }

    pub fn snapshot(&self, instance: &ServiceInstance) -> Option<InstanceLoadSnapshot> {
        self.states
            .get(&instance_identity(instance))
            .map(|state| state.snapshot())
    }

    fn state_for(&self, key: &str, now_nanos: u64) -> Arc<InstanceLoad> {
        self.states
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(InstanceLoad::new(self.config, now_nanos)))
            .clone()
    }

    fn choose<'a>(
        &self,
        instances: &'a [ServiceInstance],
        now_nanos: u64,
    ) -> Option<&'a ServiceInstance> {
        match instances.len() {
            0 => None,
            1 => instances.first(),
            len => {
                let seed = self.cursor.fetch_add(1, Ordering::Relaxed) as usize;
                let first_idx = mix_index(seed, len);
                let second_idx = mix_index(seed.wrapping_add(0x9e37_79b9), len - 1);
                let second_idx = if second_idx >= first_idx {
                    second_idx + 1
                } else {
                    second_idx
                };
                let first = &instances[first_idx];
                let second = &instances[second_idx];
                let first_state = self.state_for(&instance_identity(first), now_nanos);
                let second_state = self.state_for(&instance_identity(second), now_nanos);
                let force_after = duration_nanos(self.config.force_pick_after);
                let first_starved = now_nanos
                    .saturating_sub(first_state.last_picked_nanos.load(Ordering::Relaxed))
                    > force_after;
                let second_starved = now_nanos
                    .saturating_sub(second_state.last_picked_nanos.load(Ordering::Relaxed))
                    > force_after;
                if first_starved != second_starved {
                    return Some(if first_starved { first } else { second });
                }
                Some(if first_state.load() <= second_state.load() {
                    first
                } else {
                    second
                })
            }
        }
    }

    fn now_nanos(&self) -> u64 {
        duration_nanos(self.started.elapsed())
    }
}

impl Balancer for EwmaP2cBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        let now_nanos = self.now_nanos();
        self.synchronize(instances, now_nanos);
        self.choose(instances, now_nanos).cloned()
    }
}

#[derive(Debug)]
#[must_use = "an attempt lease must be finished or dropped to release in-flight state"]
pub struct AttemptLease {
    instance: ServiceInstance,
    state: Arc<InstanceLoad>,
    config: EwmaP2cConfig,
    started: Instant,
    clock_started: Instant,
    settled: bool,
}

impl AttemptLease {
    pub fn instance(&self) -> &ServiceInstance {
        &self.instance
    }

    pub fn finish(mut self, outcome: AttemptOutcome) {
        self.settle(outcome);
    }

    fn settle(&mut self, outcome: AttemptOutcome) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.state.decrement_inflight();
        let now_nanos = duration_nanos(self.clock_started.elapsed());
        let previous_updated = self
            .state
            .last_updated_nanos
            .swap(now_nanos, Ordering::AcqRel);
        let elapsed_since_sample = now_nanos.saturating_sub(previous_updated);
        let sample_latency = duration_nanos(self.started.elapsed());
        update_ewma(
            &self.state.ewma_latency_nanos,
            sample_latency,
            elapsed_since_sample,
            self.config.decay,
            previous_updated == 0,
        );
        let success_sample = match outcome {
            AttemptOutcome::Success => 1_000,
            AttemptOutcome::Failure | AttemptOutcome::Timeout | AttemptOutcome::Cancelled => 0,
        };
        update_ewma(
            &self.state.success_per_mille,
            success_sample,
            elapsed_since_sample,
            self.config.decay,
            previous_updated == 0,
        );
    }
}

impl Drop for AttemptLease {
    fn drop(&mut self) {
        self.settle(AttemptOutcome::Cancelled);
    }
}

#[derive(Debug, Default, Clone)]
pub struct RoundRobinBalancer {
    cursor: Arc<AtomicUsize>,
}

impl Balancer for RoundRobinBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        if instances.is_empty() {
            return None;
        }

        let idx = self.cursor.fetch_add(1, Ordering::Relaxed) % instances.len();
        Some(instances[idx].clone())
    }
}

#[derive(Debug, Default, Clone)]
pub struct FirstAvailableBalancer;

impl Balancer for FirstAvailableBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        instances.first().cloned()
    }
}

#[derive(Debug, Default, Clone)]
pub struct WeightedRoundRobinBalancer {
    cursor: Arc<AtomicUsize>,
}

impl Balancer for WeightedRoundRobinBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        let weighted = weighted_instances(instances);
        if weighted.is_empty() {
            return None;
        }

        let idx = self.cursor.fetch_add(1, Ordering::Relaxed) % weighted.len();
        Some(weighted[idx].clone())
    }
}

#[derive(Debug, Default, Clone)]
pub struct PowerOfTwoChoicesBalancer {
    cursor: Arc<AtomicU64>,
}

impl Balancer for PowerOfTwoChoicesBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        if instances.is_empty() {
            return None;
        }
        if instances.len() == 1 {
            return Some(instances[0].clone());
        }

        let seed = self.cursor.fetch_add(1, Ordering::Relaxed) as usize;
        let first_idx = mix_index(seed, instances.len());
        let second_idx = mix_index(seed.wrapping_add(0x9e37_79b9), instances.len() - 1);
        let second_idx = if second_idx >= first_idx {
            second_idx + 1
        } else {
            second_idx
        };
        let first = &instances[first_idx];
        let second = &instances[second_idx];
        if instance_score(first) >= instance_score(second) {
            Some(first.clone())
        } else {
            Some(second.clone())
        }
    }
}

fn mix_index(seed: usize, len: usize) -> usize {
    debug_assert!(len > 0);
    let mut value = seed as u64;
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^= value >> 33;
    value as usize % len
}

fn instance_identity(instance: &ServiceInstance) -> String {
    format!("{}\0{}", instance.name, instance.addr)
}

fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn update_ewma(
    target: &AtomicU64,
    sample: u64,
    elapsed_since_sample: u64,
    decay: Duration,
    first_sample: bool,
) {
    let decay_nanos = duration_nanos(decay).max(1);
    let historical_weight = if first_sample {
        0.0
    } else {
        (-(elapsed_since_sample as f64) / decay_nanos as f64).exp()
    };
    let mut previous = target.load(Ordering::Relaxed);
    loop {
        let next = ((previous as f64 * historical_weight)
            + (sample as f64 * (1.0 - historical_weight)))
            .round()
            .clamp(0.0, u64::MAX as f64) as u64;
        match target.compare_exchange_weak(previous, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => previous = actual,
        }
    }
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut left = 1u128;
    let mut right = value.min(u64::MAX as u128);
    while left <= right {
        let middle = left + (right - left) / 2;
        let quotient = value / middle;
        if middle == quotient {
            return middle;
        }
        if middle < quotient {
            left = middle + 1;
        } else {
            right = middle - 1;
        }
    }
    right
}

#[derive(Debug, Default, Clone)]
pub struct HealthAwareBalancer;

impl Balancer for HealthAwareBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        instances
            .iter()
            .max_by_key(|instance| instance_score(instance))
            .cloned()
    }
}

pub fn build_balancer(kind: BalancerKind) -> Box<dyn Balancer> {
    match kind {
        BalancerKind::FirstAvailable => Box::new(FirstAvailableBalancer),
        BalancerKind::RoundRobin => Box::new(RoundRobinBalancer::default()),
        BalancerKind::WeightedRoundRobin => Box::new(WeightedRoundRobinBalancer::default()),
        BalancerKind::PowerOfTwoChoices => Box::new(PowerOfTwoChoicesBalancer::default()),
        BalancerKind::HealthAware => Box::new(HealthAwareBalancer),
    }
}

pub fn pick(kind: BalancerKind, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
    build_balancer(kind).pick(instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_round_robin_honors_weights() {
        let mut a = ServiceInstance::new("user", "127.0.0.1:8080");
        a.weight = 2;
        let mut b = ServiceInstance::new("user", "127.0.0.1:8081");
        b.weight = 1;
        let balancer = WeightedRoundRobinBalancer::default();

        let first = balancer.pick(&[a.clone(), b.clone()]).expect("pick");
        let second = balancer.pick(&[a.clone(), b.clone()]).expect("pick");
        let third = balancer.pick(&[a.clone(), b.clone()]).expect("pick");

        assert_eq!(first.addr, a.addr);
        assert_eq!(second.addr, a.addr);
        assert_eq!(third.addr, b.addr);
    }

    #[test]
    fn health_aware_prefers_healthy_instances() {
        let mut a = ServiceInstance::new("user", "127.0.0.1:8080");
        a.metadata.insert("healthy".into(), "false".into());
        a.metadata.insert("load".into(), "99".into());

        let mut b = ServiceInstance::new("user", "127.0.0.1:8081");
        b.metadata.insert("healthy".into(), "true".into());
        b.metadata.insert("load".into(), "1".into());

        let balancer = HealthAwareBalancer;
        let picked = balancer.pick(&[a, b.clone()]).expect("pick");
        assert_eq!(picked.addr, b.addr);
    }

    #[test]
    fn p2c_prefers_better_score() {
        let mut a = ServiceInstance::new("user", "127.0.0.1:8080");
        a.metadata.insert("load".into(), "100".into());
        let mut b = ServiceInstance::new("user", "127.0.0.1:8081");
        b.metadata.insert("load".into(), "1".into());
        let balancer = PowerOfTwoChoicesBalancer::default();
        let picked = balancer.pick(&[a.clone(), b.clone()]).expect("pick");
        assert_eq!(picked.addr, b.addr);
    }

    #[test]
    fn ewma_p2c_tracks_attempt_completion_and_drop() {
        let balancer = EwmaP2cBalancer::new(EwmaP2cConfig {
            decay: Duration::from_nanos(1),
            ..EwmaP2cConfig::default()
        });
        let instance = ServiceInstance::new("user", "127.0.0.1:8080");

        let lease = balancer
            .pick_tracked(std::slice::from_ref(&instance))
            .expect("lease");
        assert_eq!(balancer.snapshot(&instance).expect("state").inflight, 1);
        lease.finish(AttemptOutcome::Success);
        assert_eq!(balancer.snapshot(&instance).expect("state").inflight, 0);

        let dropped = balancer
            .pick_tracked(std::slice::from_ref(&instance))
            .expect("dropped lease");
        assert_eq!(balancer.snapshot(&instance).expect("state").inflight, 1);
        drop(dropped);
        let snapshot = balancer.snapshot(&instance).expect("state");
        assert_eq!(snapshot.inflight, 0);
        assert!(snapshot.success_per_mille < 1_000);
    }

    #[test]
    fn ewma_p2c_prefers_fast_successful_instance() {
        let balancer = EwmaP2cBalancer::new(EwmaP2cConfig {
            decay: Duration::from_nanos(1),
            force_pick_after: Duration::from_secs(60),
            ..EwmaP2cConfig::default()
        });
        let slow = ServiceInstance::new("user", "127.0.0.1:8080");
        let fast = ServiceInstance::new("user", "127.0.0.1:8081");
        balancer.synchronize(&[slow.clone(), fast.clone()], balancer.now_nanos());

        let slow_state = balancer.state_for(&instance_identity(&slow), balancer.now_nanos());
        slow_state.ewma_latency_nanos.store(
            duration_nanos(Duration::from_millis(500)),
            Ordering::Relaxed,
        );
        slow_state.success_per_mille.store(250, Ordering::Relaxed);
        let fast_state = balancer.state_for(&instance_identity(&fast), balancer.now_nanos());
        fast_state
            .ewma_latency_nanos
            .store(duration_nanos(Duration::from_millis(5)), Ordering::Relaxed);
        fast_state.success_per_mille.store(1_000, Ordering::Relaxed);

        for _ in 0..16 {
            assert_eq!(
                balancer
                    .pick(&[slow.clone(), fast.clone()])
                    .expect("pick")
                    .addr,
                fast.addr
            );
        }
    }

    #[test]
    fn ewma_p2c_prunes_instances_after_grace_period() {
        let balancer = EwmaP2cBalancer::new(EwmaP2cConfig {
            stale_after: Duration::ZERO,
            ..EwmaP2cConfig::default()
        });
        let instance = ServiceInstance::new("user", "127.0.0.1:8080");
        assert!(balancer
            .pick_tracked(std::slice::from_ref(&instance))
            .is_some());
        assert_eq!(balancer.state_len(), 1);
        std::thread::sleep(Duration::from_millis(1));
        balancer.prune();
        assert_eq!(balancer.state_len(), 0);
    }

    #[test]
    fn integer_sqrt_is_bounded_and_monotonic() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(1), 1);
        assert_eq!(integer_sqrt(4), 2);
        assert_eq!(integer_sqrt(15), 3);
        assert_eq!(integer_sqrt(16), 4);
        assert!(integer_sqrt(u64::MAX as u128) <= u32::MAX as u128);
    }
}
