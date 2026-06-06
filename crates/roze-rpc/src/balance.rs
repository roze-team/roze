use std::sync::{
    atomic::{AtomicUsize, Ordering},
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
    cursor: Arc<AtomicUsize>,
}

impl Balancer for PowerOfTwoChoicesBalancer {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance> {
        if instances.is_empty() {
            return None;
        }
        if instances.len() == 1 {
            return Some(instances[0].clone());
        }

        let first_idx = self.cursor.fetch_add(1, Ordering::Relaxed) % instances.len();
        let second_idx = self.cursor.fetch_add(1, Ordering::Relaxed) % instances.len();
        let first = &instances[first_idx];
        let second = &instances[second_idx];
        if instance_score(first) >= instance_score(second) {
            Some(first.clone())
        } else {
            Some(second.clone())
        }
    }
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
        BalancerKind::HealthAware => Box::new(HealthAwareBalancer::default()),
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

        let balancer = HealthAwareBalancer::default();
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
