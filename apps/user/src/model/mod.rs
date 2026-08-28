//! Generated model context hook.

use crate::svc::ServiceContext;

pub async fn configure_context(ctx: ServiceContext) -> anyhow::Result<ServiceContext> {
    Ok(ctx)
}
