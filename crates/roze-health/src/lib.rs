use std::fmt::{self, Display};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    pub fn is_alive(self) -> bool {
        !matches!(self, HealthStatus::Unhealthy)
    }
}

impl Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthStatus::Healthy => f.write_str("healthy"),
            HealthStatus::Degraded => f.write_str("degraded"),
            HealthStatus::Unhealthy => f.write_str("unhealthy"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
    pub message: Option<String>,
}

impl HealthCheck {
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            message: None,
        }
    }

    pub fn degraded(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
        }
    }

    pub fn unhealthy(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub checks: Vec<HealthCheck>,
}

impl HealthReport {
    pub fn new(checks: Vec<HealthCheck>) -> Self {
        Self { checks }
    }

    pub fn overall_status(&self) -> HealthStatus {
        if self
            .checks
            .iter()
            .any(|check| matches!(check.status, HealthStatus::Unhealthy))
        {
            HealthStatus::Unhealthy
        } else if self
            .checks
            .iter()
            .any(|check| matches!(check.status, HealthStatus::Degraded))
        {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    pub fn is_ready(&self) -> bool {
        self.overall_status().is_ready()
    }

    pub fn is_alive(&self) -> bool {
        self.overall_status().is_alive()
    }

    pub fn render_text(&self) -> String {
        let mut out = format!("status={}\n", self.overall_status());
        for check in &self.checks {
            out.push_str(&format!("{}={}", check.name, check.status));
            if let Some(message) = &check.message {
                out.push_str(&format!(" message={message}"));
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_statuses() {
        let report = HealthReport::new(vec![
            HealthCheck::healthy("db"),
            HealthCheck::degraded("cache", "warmup"),
        ]);

        assert_eq!(report.overall_status(), HealthStatus::Degraded);
        assert!(report.is_alive());
        assert!(!report.is_ready());
        assert!(report.render_text().contains("cache=degraded"));
    }
}
