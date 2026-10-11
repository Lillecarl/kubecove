use super::{arg, serialize};
use crate::{backend::BackendState, channels::ChannelHub};
use kubecove_lib::{
    commands,
    models::{
        AppError, ArgoApplicationRef, ArgoConfirmedTarget, ArgoConnectionProfile,
        ArgoManagedResource, ArgoOperationConfirmation, ArgoOperationRequest, ArgoServerEndpoint,
        YamlEncoding, YamlViewMode,
    },
    runtime::State,
};
use serde_json::Value;
use std::sync::Arc;

command_table! { state, _channels, args;
    detect_argocd => commands::detect_argocd(arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?).await?,
    list_argocd_applications => commands::list_argocd_applications(arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?).await?,
    get_argocd_application_details => commands::get_argocd_application_details(arg::<String>(args, "clusterContext")?, arg::<String>(args, "name")?, arg::<Option<String>>(args, "namespace")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<YamlViewMode>>(args, "yamlViewMode")?, arg::<Option<YamlEncoding>>(args, "yamlEncoding")?).await?,
    list_argocd_appsets => commands::list_argocd_appsets(arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?).await?,
    list_argocd_appprojects => commands::list_argocd_appprojects(arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?).await?,
    get_argocd_appset_details => commands::get_argocd_appset_details(arg::<String>(args, "clusterContext")?, arg::<String>(args, "name")?, arg::<Option<String>>(args, "namespace")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<YamlViewMode>>(args, "yamlViewMode")?, arg::<Option<YamlEncoding>>(args, "yamlEncoding")?).await?,
    get_argocd_appproject_details => commands::get_argocd_appproject_details(arg::<String>(args, "clusterContext")?, arg::<String>(args, "name")?, arg::<Option<String>>(args, "namespace")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<YamlViewMode>>(args, "yamlViewMode")?, arg::<Option<YamlEncoding>>(args, "yamlEncoding")?).await?,
    discover_argo_servers => commands::discover_argo_servers(State::new(&state.cancellation), arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<String>>(args, "requestId")?, arg::<Option<String>>(args, "cancelScope")?).await?,
    connect_argo_server => commands::connect_argo_server(State::new(&state.argo), arg::<String>(args, "id")?, arg::<String>(args, "serverUrl")?, arg::<Option<ArgoServerEndpoint>>(args, "endpoint")?, arg::<Option<String>>(args, "token")?, arg::<Option<String>>(args, "username")?, arg::<Option<String>>(args, "password")?, arg::<bool>(args, "insecureTls")?, arg::<Option<Vec<u8>>>(args, "customCaPem")?, arg::<bool>(args, "rememberCredential")?, arg::<Option<String>>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<String>>(args, "workspaceId")?, arg::<Option<ArgoConfirmedTarget>>(args, "confirmedTarget")?).await?,
    get_argo_connection_status => commands::get_argo_connection_status(State::new(&state.argo), arg::<String>(args, "id")?)?,
    disconnect_argo_server => commands::disconnect_argo_server(State::new(&state.argo), arg::<String>(args, "id")?).await?,
    forget_argo_credential => commands::forget_argo_credential(arg::<ArgoConnectionProfile>(args, "profile")?)?,
    get_argo_application_inspector => commands::get_argo_application_inspector(State::new(&state.argo), State::new(&state.cancellation), arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<String>>(args, "connectionId")?, arg::<String>(args, "transport")?, arg::<ArgoApplicationRef>(args, "application")?, arg::<Option<bool>>(args, "redactSecrets")?, arg::<Option<String>>(args, "requestId")?, arg::<Option<String>>(args, "cancelScope")?).await?,
    get_argo_resource_comparison => commands::get_argo_resource_comparison(State::new(&state.argo), State::new(&state.cancellation), arg::<String>(args, "clusterContext")?, arg::<Option<String>>(args, "kubeconfigEnvVar")?, arg::<Option<String>>(args, "connectionId")?, arg::<String>(args, "transport")?, arg::<ArgoApplicationRef>(args, "application")?, arg::<ArgoManagedResource>(args, "resource")?, arg::<Option<bool>>(args, "redactSecrets")?, arg::<Option<String>>(args, "requestId")?, arg::<Option<String>>(args, "cancelScope")?).await?,
    preflight_argo_operation => commands::preflight_argo_operation(State::new(&state.argo), arg::<ArgoOperationRequest>(args, "request")?).await?,
    run_argo_operation => commands::run_argo_operation(State::new(&state.argo), arg::<ArgoOperationConfirmation>(args, "confirmation")?).await?
}
