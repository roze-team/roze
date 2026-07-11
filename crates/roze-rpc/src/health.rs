use std::{future::Future, time::Duration};

use roze_grpc::transport::NamedService;
use roze_health::{HealthRegistry, HealthReport};
use tonic_health::{
    pb::health_server::HealthServer,
    server::{HealthReporter, HealthService},
    ServingStatus,
};

pub type GrpcHealthService = HealthServer<HealthService>;

#[derive(Clone, Debug)]
pub struct RpcHealthReporter {
    registry: HealthRegistry,
    reporter: HealthReporter,
    service_name: &'static str,
}

impl RpcHealthReporter {
    pub fn new_for<S>(registry: HealthRegistry) -> (Self, GrpcHealthService)
    where
        S: NamedService,
    {
        let reporter = HealthReporter::new();
        let service = HealthServer::new(HealthService::from_health_reporter(reporter.clone()));
        (
            Self {
                registry,
                reporter,
                service_name: S::NAME,
            },
            service,
        )
    }

    pub async fn refresh(&self) -> HealthReport {
        let report = self.registry.readiness_report().await;
        let status = serving_status(&report);
        self.reporter.set_service_status("", status).await;
        self.reporter
            .set_service_status(self.service_name, status)
            .await;
        report
    }

    pub async fn run_until<F>(&self, refresh_interval: Duration, shutdown: F)
    where
        F: Future<Output = ()>,
    {
        assert!(
            !refresh_interval.is_zero(),
            "RPC health refresh interval must be positive"
        );
        let mut ticker = tokio::time::interval(refresh_interval);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.refresh().await;
                }
                _ = &mut shutdown => {
                    self.registry.mark_draining();
                    self.refresh().await;
                    return;
                }
            }
        }
    }
}

fn serving_status(report: &HealthReport) -> ServingStatus {
    if report.is_ready() {
        ServingStatus::Serving
    } else {
        ServingStatus::NotServing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic_health::{
        pb::{health_server::Health, HealthCheckRequest},
        server::HealthService,
    };

    struct ExampleService;

    impl NamedService for ExampleService {
        const NAME: &'static str = "roze.test.Example";
    }

    async fn status(reporter: &RpcHealthReporter, service_name: &str) -> i32 {
        let service = HealthService::from_health_reporter(reporter.reporter.clone());
        service
            .check(roze_grpc::transport::Request::new(HealthCheckRequest {
                service: service_name.to_string(),
            }))
            .await
            .expect("health check")
            .into_inner()
            .status
    }

    fn wire_status(status: ServingStatus) -> i32 {
        tonic_health::pb::health_check_response::ServingStatus::from(status) as i32
    }

    #[tokio::test]
    async fn refresh_maps_registry_readiness_to_overall_and_service_status() {
        let registry = HealthRegistry::new();
        let (reporter, _) = RpcHealthReporter::new_for::<ExampleService>(registry.clone());

        reporter.refresh().await;
        assert_eq!(
            status(&reporter, "").await,
            wire_status(ServingStatus::NotServing)
        );
        assert_eq!(
            status(&reporter, ExampleService::NAME).await,
            wire_status(ServingStatus::NotServing)
        );

        registry.mark_ready();
        reporter.refresh().await;
        assert_eq!(
            status(&reporter, "").await,
            wire_status(ServingStatus::Serving)
        );
        assert_eq!(
            status(&reporter, ExampleService::NAME).await,
            wire_status(ServingStatus::Serving)
        );
    }

    #[tokio::test]
    async fn shutdown_marks_registry_and_grpc_service_not_serving() {
        let registry = HealthRegistry::new();
        registry.mark_ready();
        let (reporter, _) = RpcHealthReporter::new_for::<ExampleService>(registry.clone());

        reporter
            .run_until(Duration::from_secs(60), std::future::ready(()))
            .await;

        assert_eq!(registry.phase(), roze_health::ServicePhase::Draining);
        assert_eq!(
            status(&reporter, ExampleService::NAME).await,
            wire_status(ServingStatus::NotServing)
        );
    }
}
