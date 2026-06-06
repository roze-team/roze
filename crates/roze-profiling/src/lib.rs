use std::time::{Duration, Instant};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_sample() {
        let sample = ProfileMark::start("boot").finish();
        assert_eq!(sample.name, "boot");
    }
}
