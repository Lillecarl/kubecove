use crate::{access::AccessIdentity, ApiError};
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use kubecove_lib::models::AppErrorKind;
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub const TRANSACTION_COOKIE: &str = "__Host-kubecove_oidc_tx";
pub const AUTH_COOKIE: &str = "__Host-kubecove_oidc";

#[derive(Clone)]
pub struct OidcConfig {
    issuer: String,
    client_id: String,
    secret_file: Option<PathBuf>,
    login_timeout: Duration,
}

impl OidcConfig {
    pub fn new(
        issuer: String,
        client_id: String,
        secret_file: Option<PathBuf>,
        login_timeout: Duration,
    ) -> Result<Self, String> {
        validate_issuer(&issuer)?;
        if client_id.trim().is_empty() {
            return Err("OIDC client ID must not be empty".into());
        }
        if login_timeout.is_zero()
            || std::time::Instant::now()
                .checked_add(login_timeout)
                .is_none()
        {
            return Err("OIDC login timeout must be positive and representable".into());
        }
        Ok(Self {
            issuer,
            client_id,
            secret_file,
            login_timeout,
        })
    }
}

pub struct AuthSession {
    pub identity: AccessIdentity,
    pub revoked: CancellationToken,
}

pub struct Oidc {
    config: OidcConfig,
    redirect_uri: String,
    client_secret: Option<String>,
    http: reqwest::Client,
    metadata: CoreProviderMetadata,
    transaction_key: [u8; 32],
    credentials: Mutex<HashMap<String, Credential>>,
    allow_http_test: bool,
}

struct Credential {
    identity: AccessIdentity,
    expires_unix: u64,
    revoked: CancellationToken,
}

#[derive(Serialize, Deserialize)]
struct Transaction {
    state: String,
    nonce: String,
    verifier: String,
    #[serde(rename = "exp")]
    expires_at: u64,
}

#[derive(Clone, Deserialize)]
pub struct Callback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct TransactionClaims {
    state: String,
    nonce: String,
    verifier: String,
    #[serde(rename = "exp")]
    exp: u64,
}

impl Oidc {
    pub async fn discover(config: OidcConfig, public_origin: &str) -> Result<Self, String> {
        let origin = validate_public_origin(public_origin)?;
        let http = client(config.login_timeout, true)?;
        let metadata = discover_metadata(&config.issuer, &http).await?;
        validate_metadata(&config.issuer, &metadata, false)?;
        let client_secret = config
            .secret_file
            .as_ref()
            .map(fs::read_to_string)
            .transpose()
            .map_err(|_| "failed to read OIDC client secret file")?
            .map(|secret| secret.trim_end_matches(['\r', '\n']).to_owned());
        if client_secret.as_deref().is_some_and(str::is_empty) {
            return Err("OIDC client secret file is empty".into());
        }
        let mut transaction_key = [0; 32];
        getrandom::fill(&mut transaction_key)
            .map_err(|_| "failed to initialize OIDC transaction signing")?;
        let redirect_uri = format!("{origin}/api/v1/oidc/callback");
        Ok(Self {
            config,
            redirect_uri,
            client_secret,
            http,
            metadata,
            transaction_key,
            credentials: Mutex::new(HashMap::new()),
            allow_http_test: false,
        })
    }

    pub fn start(&self, _headers: &HeaderMap) -> Result<Response, ApiError> {
        let client = self
            .client(&self.metadata)
            .map_err(|_| oidc_unavailable())?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (auth_url, state, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge)
            .url();
        let expires_at = unix_now()
            .checked_add(timeout_seconds_ceil(self.config.login_timeout))
            .ok_or_else(oidc_unavailable)?;
        let transaction = Transaction {
            state: state.secret().to_owned(),
            nonce: nonce.secret().to_owned(),
            verifier: verifier.secret().to_owned(),
            expires_at,
        };
        let signed = sign_transaction(&transaction, &self.transaction_key)
            .map_err(|_| oidc_unavailable())?;
        let mut response = (
            StatusCode::OK,
            Json(serde_json::json!({ "authorizationUrl": auth_url.as_str() })),
        )
            .into_response();
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie_header(&transaction_cookie(&signed))?,
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }

    pub async fn callback(
        &self,
        headers: &HeaderMap,
        query: Callback,
    ) -> Result<Response, ApiError> {
        match self.callback_inner(headers, query).await {
            Ok((issuer, subject, exp)) => match self.callback_success(&issuer, &subject, exp) {
                Ok(response) => Ok(response),
                Err(_) => Ok(callback_failure(false)),
            },
            Err(error) => Ok(callback_failure(matches!(error.0, StatusCode::SEE_OTHER))),
        }
    }

    fn callback_success(
        &self,
        issuer: &str,
        subject: &str,
        exp: u64,
    ) -> Result<Response, ApiError> {
        let opaque = crate::security::random_token().map_err(|_| oidc_unavailable())?;
        let expiry = UNIX_EPOCH
            .checked_add(Duration::from_secs(exp))
            .ok_or_else(oidc_unavailable)?;
        let remaining = expiry
            .duration_since(SystemTime::now())
            .map_err(|_| invalid_callback())?;
        if remaining.is_zero() {
            return Err(invalid_callback());
        }
        let identity = AccessIdentity {
            subject: format!("{issuer}\u{1f}{subject}"),
            expires_at: Instant::now()
                .checked_add(remaining)
                .ok_or_else(oidc_unavailable)?,
        };
        let credential = Credential {
            identity,
            expires_unix: exp,
            revoked: CancellationToken::new(),
        };
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        prune_expired(&mut credentials, unix_now());
        credentials.insert(opaque.clone(), credential);
        let mut response = Redirect::to("/").into_response();
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie_header(&auth_cookie(&opaque))?);
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie_header(&clear_cookie_header(TRANSACTION_COOKIE))?,
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }

    async fn callback_inner(
        &self,
        headers: &HeaderMap,
        query: Callback,
    ) -> Result<(String, String, u64), ApiError> {
        if query.error.is_some() {
            return Err(ApiError::new(
                StatusCode::SEE_OTHER,
                "OIDC sign-in was not completed",
                AppErrorKind::Forbidden,
            ));
        }
        let state = query.state.as_deref().ok_or_else(invalid_callback)?;
        let code = query
            .code
            .filter(|code| !code.is_empty())
            .ok_or_else(invalid_callback)?;
        let signed = cookie_value(headers, TRANSACTION_COOKIE).ok_or_else(invalid_callback)?;
        let transaction = verify_transaction(signed, &self.transaction_key, unix_now())
            .map_err(|_| invalid_callback())?;
        if !constant_time_state_matches(state, &transaction.state) {
            return Err(invalid_callback());
        }

        // Discovery is deliberately repeated for each callback so rotated signing keys are used.
        let metadata = discover_metadata(&self.config.issuer, &self.http)
            .await
            .map_err(|_| oidc_unavailable())?;
        validate_metadata(&self.config.issuer, &metadata, self.allow_http_test)
            .map_err(|_| oidc_unavailable())?;
        let client = self.client(&metadata).map_err(|_| oidc_unavailable())?;
        let response = client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|_| invalid_callback())?
            .set_pkce_verifier(PkceCodeVerifier::new(transaction.verifier))
            .request_async(&{
                let http = self.http.clone();
                move |request| send_request(http.clone(), request)
            })
            .await
            .map_err(|_| oidc_unavailable())?;
        let id_token = response.id_token().ok_or_else(invalid_callback)?;
        let nonce = Nonce::new(transaction.nonce);
        let verifier = client
            .id_token_verifier()
            .set_issue_time_verifier_fn(|issued_at| {
                if issued_at.timestamp() > i64::try_from(unix_now()).unwrap_or(i64::MAX) {
                    Err("ID token issue time is in the future".to_owned())
                } else {
                    Ok(())
                }
            });
        let claims = id_token
            .claims(&verifier, &nonce)
            .map_err(|_| invalid_callback())?;
        let client_id = self.config.client_id.as_str();
        if claims
            .authorized_party()
            .is_some_and(|party| party.as_str() != client_id)
            || (claims.audiences().len() > 1
                && claims
                    .authorized_party()
                    .is_none_or(|party| party.as_str() != client_id))
            || id_token_has_invalid_not_before(&id_token.to_string())
        {
            return Err(invalid_callback());
        }
        let subject = claims.subject().as_str();
        if subject.is_empty() {
            return Err(invalid_callback());
        }
        let exp = claims
            .expiration()
            .timestamp()
            .try_into()
            .map_err(|_| invalid_callback())?;
        if exp <= unix_now() {
            return Err(invalid_callback());
        }
        Ok((self.config.issuer.clone(), subject.to_owned(), exp))
    }

    pub fn verify(&self, headers: &HeaderMap) -> Result<AuthSession, ApiError> {
        let token = cookie_value(headers, AUTH_COOKIE).ok_or_else(invalid_auth)?;
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        prune_expired(&mut credentials, unix_now());
        let credential = credentials.get(token).ok_or_else(invalid_auth)?;
        Ok(AuthSession {
            identity: credential.identity.clone(),
            revoked: credential.revoked.clone(),
        })
    }

    pub fn logout(&self, headers: &HeaderMap) -> Result<Option<String>, ApiError> {
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        let subject = cookie_value(headers, AUTH_COOKIE)
            .and_then(|token| credentials.remove(token))
            .map(|credential| {
                credential.revoked.cancel();
                credential.identity.subject
            });
        prune_expired(&mut credentials, unix_now());
        Ok(subject)
    }

    pub fn clear_cookies(&self) -> Result<Vec<HeaderValue>, ApiError> {
        [TRANSACTION_COOKIE, AUTH_COOKIE]
            .into_iter()
            .map(|name| cookie_header(&clear_cookie_header(name)))
            .collect()
    }

    fn client(
        &self,
        metadata: &CoreProviderMetadata,
    ) -> Result<
        CoreClient<
            EndpointSet,
            EndpointNotSet,
            EndpointNotSet,
            EndpointNotSet,
            EndpointMaybeSet,
            EndpointMaybeSet,
        >,
        String,
    > {
        let secret = self
            .client_secret
            .as_ref()
            .map(|secret| ClientSecret::new(secret.clone()));
        Ok(CoreClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(self.config.client_id.clone()),
            secret,
        )
        .set_redirect_uri(
            RedirectUrl::new(self.redirect_uri.clone()).map_err(|_| "invalid OIDC redirect URI")?,
        ))
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(config: OidcConfig, origin: &str) -> Result<Self, String> {
        let origin = test_origin(origin)?;
        let metadata = test_metadata(&config.issuer)?;
        let mut key = [0; 32];
        getrandom::fill(&mut key).map_err(|_| "failed test key")?;
        // The test-only constructor is used for registry, cookie and signature tests.
        Ok(Self {
            config,
            redirect_uri: format!("{origin}/api/v1/oidc/callback"),
            client_secret: None,
            http: client(Duration::from_secs(30), false)?,
            metadata,
            transaction_key: key,
            credentials: Mutex::new(HashMap::new()),
            allow_http_test: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn insert_identity_for_test(&self, issuer: &str, subject: &str, exp: u64) -> String {
        let token = crate::security::random_token().unwrap();
        let remaining = Duration::from_secs(exp.saturating_sub(unix_now()));
        self.credentials.lock().unwrap().insert(
            token.clone(),
            Credential {
                identity: AccessIdentity {
                    subject: format!("{issuer}\u{1f}{subject}"),
                    expires_at: Instant::now() + remaining,
                },
                expires_unix: exp,
                revoked: CancellationToken::new(),
            },
        );
        token
    }

    #[cfg(test)]
    pub(crate) fn revoke_for_test(&self, token: &str) {
        if let Some(credential) = self.credentials.lock().unwrap().remove(token) {
            credential.revoked.cancel();
        }
    }
}

fn authority_has_userinfo(value: &str) -> bool {
    value
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'))
}

fn validate_issuer(issuer: &str) -> Result<(), String> {
    let url = openidconnect::url::Url::parse(issuer)
        .map_err(|_| "OIDC issuer must be a valid HTTPS URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || authority_has_userinfo(issuer)
    {
        return Err("OIDC issuer must be HTTPS without credentials, query, or fragment".into());
    }
    Ok(())
}

pub(crate) fn validate_public_origin(origin: &str) -> Result<String, String> {
    let url = openidconnect::url::Url::parse(origin)
        .map_err(|_| "OIDC public origin must be an exact HTTPS origin")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || authority_has_userinfo(origin)
    {
        return Err("OIDC public origin must be an exact HTTPS origin".into());
    }
    Ok(format!(
        "https://{}{}",
        url.host_str().unwrap_or_default(),
        url.port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default()
    ))
}

async fn discover_metadata(
    issuer: &str,
    http: &reqwest::Client,
) -> Result<CoreProviderMetadata, String> {
    let issuer = IssuerUrl::new(issuer.to_owned()).map_err(|_| "invalid OIDC issuer")?;
    let http = http.clone();
    CoreProviderMetadata::discover_async(issuer, &move |request| {
        send_request(http.clone(), request)
    })
    .await
    .map_err(|_| "OIDC provider discovery failed".into())
}

#[cfg(test)]
fn test_origin(origin: &str) -> Result<String, String> {
    if origin.starts_with("https://") {
        return validate_public_origin(origin);
    }
    crate::oidc::mock::test_loopback_origin(origin)
}

fn validate_metadata(
    issuer: &str,
    metadata: &CoreProviderMetadata,
    allow_loopback_http: bool,
) -> Result<(), String> {
    if metadata.issuer().as_str() != issuer {
        return Err("OIDC provider issuer does not match configured issuer".into());
    }
    let token_endpoint = metadata
        .token_endpoint()
        .ok_or("OIDC provider omitted its token endpoint")?;
    let endpoints = [
        metadata.authorization_endpoint().url(),
        token_endpoint.url(),
        metadata.jwks_uri().url(),
    ];
    for endpoint in endpoints {
        let url = openidconnect::url::Url::parse(endpoint.as_str())
            .map_err(|_| "invalid OIDC metadata endpoint")?;
        let local_http = allow_loopback_http
            && url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
        if (url.scheme() != "https" && !local_http) || url.host_str().is_none() {
            return Err("OIDC metadata endpoints must use HTTPS".into());
        }
    }
    Ok(())
}

fn client(timeout: Duration, https_only: bool) -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .timeout(timeout)
        .https_only(https_only)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "failed to initialize OIDC HTTP client".into())
}

async fn send_request(
    http: reqwest::Client,
    request: openidconnect::HttpRequest,
) -> Result<openidconnect::HttpResponse, reqwest::Error> {
    let (parts, body) = request.into_parts();
    let response = http
        .request(parts.method, parts.uri.to_string())
        .headers(parts.headers)
        .body(body)
        .send()
        .await?;
    let mut builder = openidconnect::http::Response::builder().status(response.status());
    *builder.headers_mut().expect("new response headers") = response.headers().clone();
    Ok(builder
        .body(response.bytes().await?.to_vec())
        .expect("valid HTTP response"))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut found = None;
    let cookies = headers.get_all(header::COOKIE).iter();
    for header in cookies {
        let value = header.to_str().ok()?;
        for entry in value.split(';').map(str::trim) {
            if let Some((key, value)) = entry.split_once('=') {
                if key == name {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(value);
                }
            }
        }
    }
    found
}

fn cookie_header(value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|_| oidc_unavailable())
}

fn transaction_cookie(value: &str) -> String {
    format!("{TRANSACTION_COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Lax")
}
fn auth_cookie(value: &str) -> String {
    format!("{AUTH_COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Strict")
}
pub fn clear_cookie_header(name: &str) -> String {
    format!("{name}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0")
}

fn id_token_has_invalid_not_before(token: &str) -> bool {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let Some(payload) = token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
    else {
        return true;
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return true;
    };
    claims.get("nbf").is_some_and(|not_before| {
        not_before
            .as_i64()
            .is_none_or(|not_before| not_before > i64::try_from(unix_now()).unwrap_or(i64::MAX))
    })
}

fn sign_transaction(
    transaction: &Transaction,
    key: &[u8],
) -> Result<String, jsonwebtoken::errors::Error> {
    encode(
        &Header::new(Algorithm::HS256),
        transaction,
        &EncodingKey::from_secret(key),
    )
}

fn verify_transaction(
    token: &str,
    key: &[u8],
    now: u64,
) -> Result<TransactionClaims, jsonwebtoken::errors::Error> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let claims =
        decode::<TransactionClaims>(token, &DecodingKey::from_secret(key), &validation)?.claims;
    if claims.exp <= now {
        return Err(jsonwebtoken::errors::ErrorKind::ExpiredSignature.into());
    }
    Ok(claims)
}

fn constant_time_state_matches(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn timeout_seconds_ceil(timeout: Duration) -> u64 {
    timeout
        .as_secs()
        .saturating_add(u64::from(timeout.subsec_nanos() > 0))
}

fn prune_expired(credentials: &mut HashMap<String, Credential>, now: u64) {
    credentials.retain(|_, credential| {
        if credential.expires_unix <= now {
            credential.revoked.cancel();
            false
        } else {
            true
        }
    });
}

fn callback_failure(denied: bool) -> Response {
    let mut response = if denied {
        Redirect::to("/").into_response()
    } else {
        StatusCode::BAD_REQUEST.into_response()
    };
    if let Ok(cookie) = HeaderValue::from_str(&clear_cookie_header(TRANSACTION_COOKIE)) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn invalid_callback() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "OIDC callback is invalid or expired",
        AppErrorKind::Forbidden,
    )
}
fn invalid_auth() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "OIDC sign-in is required or has expired",
        AppErrorKind::Forbidden,
    )
}
fn oidc_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "OIDC sign-in is temporarily unavailable",
        AppErrorKind::Internal,
    )
}

#[cfg(test)]
fn test_metadata(issuer: &str) -> Result<CoreProviderMetadata, String> {
    serde_json::from_value(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/auth"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"]
    }))
    .map_err(|_| "invalid test provider metadata".into())
}

#[cfg(test)]
#[path = "oidc_mock.rs"]
pub(crate) mod mock;
#[cfg(test)]
#[path = "oidc_tests.rs"]
mod tests;
