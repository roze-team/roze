use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::registry::{weighted_instances, ServiceInstance};

pub trait Balancer: Send + Sync + 'static {
    fn pick(&self, instances: &[ServiceInstance]) -> Option<ServiceInstance>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalancerKind {
    FirstAvailable,
    RoundRobin,
    WeightedRoundRobin,
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

pub fn build_balancer(kind: BalancerKind) -> Box<dyn Balancer> {
    match kind {
        BalancerKind::FirstAvailable => Box::new(FirstAvailableBalancer),
        BalancerKind::RoundRobin => Box::new(RoundRobinBalancer::default()),
        BalancerKind::WeightedRoundRobin => Box::new(WeightedRoundRobinBalancer::default()),
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
}
