use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct ProfileMark {
    name: String,
    started_at: Instant,
}

impl ProfileMark {
    pub fn start(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            started_at: Instant::now(),
        }
    }

    pub fn finish(self) -> ProfileSample {
        ProfileSample {
            name: self.name,
            elapsed: self.started_at.elapsed(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSample {
    pub name: String,
    pub elapsed: Duration,
}

impl ProfileSample {
    pub fn elapsed_micros(&self) -> u128 {
        self.elapsed.as_micros()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProfilingRegistry {
    samples: Arc<Mutex<Vec<ProfileSample>>>,
}

impl ProfilingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark(&self, sample: ProfileSample) {
        self.samples
            .lock()
            .expect("profiling lock poisoned")
            .push(sample);
    }

    pub fn capture(&self, name: impl Into<String>) -> ProfileGuard {
        ProfileGuard {
            registry: self.clone(),
            mark: ProfileMark::start(name),
        }
    }

    pub fn samples(&self) -> Vec<ProfileSample> {
        self.samples
            .lock()
            .expect("profiling lock poisoned")
            .clone()
    }

    pub fn summary(&self) -> BTreeMap<String, Duration> {
        let mut totals = BTreeMap::new();
        for sample in self.samples() {
            totals
                .entry(sample.name)
                .and_modify(|duration: &mut Duration| *duration += sample.elapsed)
                .or_insert(sample.elapsed);
        }
        totals
    }
}

pub struct ProfileGuard {
    registry: ProfilingRegistry,
    mark: ProfileMark,
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        let sample = self.mark.clone().finish();
        self.registry.mark(sample);
    }
}

pub fn render_profile_summary(registry: &ProfilingRegistry) -> String {
    let mut out = String::new();
    for (name, elapsed) in registry.summary() {
        out.push_str(&format!("{name}={}us\n", elapsed.as_micros()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_sample() {
        let sample = ProfileMark::start("boot").finish();
        assert_eq!(sample.name, "boot");
    }

    #[test]
    fn collects_summary() {
        let registry = ProfilingRegistry::new();
        {
            let _guard = registry.capture("boot");
        }
        assert!(render_profile_summary(&registry).contains("boot"));
    }
}
