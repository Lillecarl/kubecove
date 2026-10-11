use kubecove_lib::{
    commands::{
        cancel_workspace_requests, AppUsageMonitor, ArgoConnectionStore,
        BackendCancellationRegistry, ClusterLiveStore, PodExecRegistry, PortForwardRegistry,
        StreamRegistry,
    },
    models::AppError,
    runtime::State,
};

#[derive(Default)]
pub struct BackendState {
    pub live_store: ClusterLiveStore,
    pub cancellation: BackendCancellationRegistry,
    pub streams: StreamRegistry,
    pub port_forwards: PortForwardRegistry,
    pub pod_exec: PodExecRegistry,
    pub usage: AppUsageMonitor,
    pub argo: ArgoConnectionStore,
}

impl BackendState {
    pub async fn cleanup(&self, stop_forwards: bool) -> Result<(), AppError> {
        self.streams.stop_all();
        self.pod_exec.stop_all();
        if stop_forwards {
            self.port_forwards.stop_all();
        }
        cancel_workspace_requests(
            State::new(&self.cancellation),
            State::new(&self.live_store),
            State::new(&self.argo),
        )
        .await?;
        Ok(())
    }
}
