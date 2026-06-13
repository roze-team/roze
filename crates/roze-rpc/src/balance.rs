use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};

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
}
