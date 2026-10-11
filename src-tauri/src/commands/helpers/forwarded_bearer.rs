use http::{header::AUTHORIZATION, HeaderValue};
use kube::client::Body;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    LazyLock, RwLock,
};
use tower::{Layer, Service};

// The server's OIDC mode can send each Kubernetes request as the signed-in
// user instead of with the kubeconfig's credentials (ADR 0022 amendment).
// There is one active owner, so one slot holds that owner's bearer token.
// Clients read it per request: a refreshed token or a new owner applies to
// cached clients and watch reconnects without rebuilding them.
static ENABLED: AtomicBool = AtomicBool::new(false);
static BEARER: LazyLock<RwLock<Option<HeaderValue>>> = LazyLock::new(|| RwLock::new(None));

/// Turns forwarding on for the rest of the process. Kubeconfig credentials
/// are then never sent, and a request with an empty slot goes out anonymous.
pub fn enable_forwarded_bearer() {
    ENABLED.store(true, Ordering::SeqCst);
}

pub fn set_forwarded_bearer(token: Option<&str>) {
    let value = token.and_then(|token| {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
        value.set_sensitive(true);
        Some(value)
    });
    *BEARER.write().expect("forwarded bearer lock") = value;
}

/// The Authorization value requests carry now, for the server's tests.
#[doc(hidden)]
pub fn forwarded_bearer() -> Option<String> {
    BEARER
        .read()
        .expect("forwarded bearer lock")
        .as_ref()
        .and_then(|value| value.to_str().ok().map(str::to_owned))
}

pub(crate) fn forwarded_bearer_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

#[derive(Clone, Copy)]
pub(crate) struct ForwardedBearerLayer;

impl<S> Layer<S> for ForwardedBearerLayer {
    type Service = ForwardedBearerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ForwardedBearerService { inner }
    }
}

#[derive(Clone)]
pub(crate) struct ForwardedBearerService<S> {
    inner: S,
}

impl<S> Service<http::Request<Body>> for ForwardedBearerService<S>
where
    S: Service<http::Request<Body>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<Body>) -> Self::Future {
        if forwarded_bearer_enabled() {
            let headers = request.headers_mut();
            headers.remove(AUTHORIZATION);
            if let Some(value) = BEARER.read().expect("forwarded bearer lock").clone() {
                headers.insert(AUTHORIZATION, value);
            }
        }
        self.inner.call(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Request, Response};
    use tower::ServiceExt;

    // ENABLED and BEARER are process-wide, so one test walks every state.
    #[tokio::test]
    async fn sets_strips_or_passes_the_authorization_header() {
        async fn sent(incoming: Option<&str>) -> Option<String> {
            let (service, mut handle) =
                tower_test::mock::pair::<Request<Body>, Response<Body>>();
            let mut service = ForwardedBearerLayer.layer(service);
            let mut request = Request::builder().uri("/api");
            if let Some(value) = incoming {
                request = request.header(AUTHORIZATION, value);
            }
            let request = request.body(Body::empty()).unwrap();
            let call = tokio::spawn(async move {
                service.ready().await.unwrap().call(request).await
            });
            let (seen, reply) = handle.next_request().await.unwrap();
            reply.send_response(Response::new(Body::empty()));
            call.await.unwrap().unwrap();
            seen.headers()
                .get(AUTHORIZATION)
                .map(|value| value.to_str().unwrap().to_owned())
        }

        set_forwarded_bearer(Some("user"));
        assert_eq!(sent(Some("Bearer sa")).await.as_deref(), Some("Bearer sa"));

        enable_forwarded_bearer();
        assert_eq!(sent(Some("Bearer sa")).await.as_deref(), Some("Bearer user"));
        assert_eq!(sent(None).await.as_deref(), Some("Bearer user"));

        set_forwarded_bearer(None);
        assert_eq!(sent(Some("Bearer sa")).await, None);
    }
}
