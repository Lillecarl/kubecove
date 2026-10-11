use crate::models::AppErrorKind;
use crate::models::{AppError, PortForwardRequest, PortForwardSessionSummary};
use crate::runtime::State;
use kube::Client;

mod registry;
pub mod runner;
pub(crate) mod service;
pub(crate) mod target;

pub struct TunnelTarget {
    pub client: Client,
    pub namespace: String,
    pub pod_name: String,
    pub pod_port: u16,
}

pub fn validate_tunnel_request(request: &PortForwardRequest) -> Result<(), AppError> {
    target::validate_request(request).map(|_| ())
}

/// Validate and resolve a tunnel target through the same Pod/Service path as desktop forwards.
pub async fn resolve_tunnel_target(request: PortForwardRequest) -> Result<TunnelTarget, AppError> {
    let validated = target::validate_request(&request)?;
    let client = target::client_for_context(
        &validated.cluster_context,
        validated.kubeconfig_env_var.clone(),
    )
    .await?;
    let target = target::resolve_port_forward_target_with_client(validated, client.clone()).await?;
    Ok(TunnelTarget {
        client,
        namespace: target.namespace,
        pod_name: target.pod_name,
        pod_port: target.pod_port,
    })
}

pub use registry::PortForwardRegistry;
#[cfg(test)]
use registry::LOCAL_ADDRESS;
#[cfg(test)]
use runner::run_port_forward_session;
use runner::start_pod_port_forward_in_registry;
#[cfg(test)]
use target::validate_request;

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn start_pod_port_forward(
    request: PortForwardRequest,
    registry: State<'_, PortForwardRegistry>,
) -> Result<PortForwardSessionSummary, AppError> {
    start_pod_port_forward_in_registry(request, registry.inner()).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn stop_port_forward(
    session_id: String,
    registry: State<'_, PortForwardRegistry>,
) -> Result<bool, AppError> {
    if session_id.trim().is_empty() {
        return Err(AppError::new(
            "port-forward session id is required",
            AppErrorKind::Validation,
        ));
    }
    Ok(registry.stop(session_id.trim()))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn list_port_forwards(
    registry: State<'_, PortForwardRegistry>,
) -> Result<Vec<PortForwardSessionSummary>, AppError> {
    Ok(registry.list())
}

#[cfg(test)]
mod tests;
