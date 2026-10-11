use super::{arg, channel, serialize};
use crate::{backend::BackendState, channels::ChannelHub};
use kubecove_lib::{
    commands,
    models::{
        AggregatedLogStreamRequest, AppError, LiveSessionCleanupRequest, PodExecSessionRequest,
        PodExecTerminalSize, PodLogStreamRequest, PortForwardRequest, WatchResourceKey,
    },
    runtime::State,
};
use serde_json::Value;
use std::sync::Arc;

command_table! { state, channels, args;
    start_resource_watch => commands::start_resource_watch(arg::<String>(args, "clusterContext")?, arg::<Vec<WatchResourceKey>>(args, "keys")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, channel(args, "channel", channels)?, State::new(&state.streams), State::new(&state.live_store)).await?,
    refresh_resource_cache => commands::refresh_resource_cache(arg::<String>(args, "clusterContext")?, arg::<Vec<WatchResourceKey>>(args, "keys")?, arg::<Vec<String>>(args, "namespaces")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, State::new(&state.live_store)).await?,
    start_resource_event_watch => commands::start_resource_event_watch(arg::<String>(args, "clusterContext")?, arg::<String>(args, "kind")?, arg::<String>(args, "name")?, arg::<Option<String>>(args, "namespace")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, channel(args, "channel", channels)?, State::new(&state.streams)).await?,
    start_pod_log_stream => commands::start_pod_log_stream(arg::<PodLogStreamRequest>(args, "request")?, channel(args, "channel", channels)?, State::new(&state.streams)).await?,
    start_aggregated_log_stream => commands::start_aggregated_log_stream(arg::<AggregatedLogStreamRequest>(args, "request")?, channel(args, "channel", channels)?, State::new(&state.streams)).await?,
    stop_stream => commands::stop_stream(arg::<String>(args, "streamId")?, State::new(&state.streams)).await?,
    start_pod_port_forward => commands::start_pod_port_forward(arg::<PortForwardRequest>(args, "request")?, State::new(&state.port_forwards)).await?,
    stop_port_forward => commands::stop_port_forward(arg::<String>(args, "sessionId")?, State::new(&state.port_forwards)).await?,
    list_port_forwards => commands::list_port_forwards(State::new(&state.port_forwards)).await?,
    start_pod_exec_session => commands::start_pod_exec_session(arg::<PodExecSessionRequest>(args, "request")?, channel(args, "channel", channels)?, State::new(&state.pod_exec)).await?,
    write_pod_exec_stdin => commands::write_pod_exec_stdin(arg::<String>(args, "sessionId")?, arg::<String>(args, "data")?, State::new(&state.pod_exec)).await?,
    resize_pod_exec_terminal => commands::resize_pod_exec_terminal(arg::<String>(args, "sessionId")?, arg::<PodExecTerminalSize>(args, "size")?, State::new(&state.pod_exec)).await?,
    stop_pod_exec_session => commands::stop_pod_exec_session(arg::<String>(args, "sessionId")?, State::new(&state.pod_exec)).await?,
    list_pod_exec_sessions => commands::list_pod_exec_sessions(State::new(&state.pod_exec)).await?,
    stop_live_sessions_outside_scope => commands::stop_live_sessions_outside_scope(arg::<LiveSessionCleanupRequest>(args, "request")?, State::new(&state.port_forwards), State::new(&state.pod_exec)).await?,
    set_backend_diagnostics_enabled => commands::set_backend_diagnostics_enabled(arg::<bool>(args, "enabled")?),
    get_backend_diagnostics => commands::get_backend_diagnostics(),
    get_backend_cache_diagnostics => commands::get_backend_cache_diagnostics(State::new(&state.live_store)),
    clear_backend_diagnostics => commands::clear_backend_diagnostics(State::new(&state.live_store)),
    cancel_backend_requests => commands::cancel_backend_requests(arg::<String>(args, "cancelScope")?, State::new(&state.cancellation)),
    cancel_workspace_requests => commands::cancel_workspace_requests(State::new(&state.cancellation), State::new(&state.live_store), State::new(&state.argo)).await?
}
