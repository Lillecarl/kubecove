use super::{
    clear_cookie_header, cookie_header, cookie_value, discover_metadata, invalid_auth,
    oidc_unavailable, send_request, unix_now, validate_metadata, AuthSession, Binding, Credential,
    Oidc, OidcCredential, Tokens, AUTH_COOKIE, REFRESH_INTERVAL, REFRESH_MARGIN_SECONDS,
    TRANSACTION_COOKIE,
};
use crate::ApiError;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use kubecove_lib::{commands::SessionScope, models::AppErrorKind};
use openidconnect::RequestTokenError;
use openidconnect::{Nonce, OAuth2TokenResponse, RefreshToken, TokenResponse};
use std::{collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

enum RefreshFailure {
    Transient,
    Refused,
}

impl Oidc {
    /// Bind only after renewal and while the caller holds the owner's request gate.
    pub fn bind(
        &self,
        credential: &OidcCredential,
        owner: &str,
        scope: Arc<SessionScope>,
        ended: &CancellationToken,
    ) -> Result<(), ApiError> {
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        self.prune_expired(&mut credentials);
        let entry = credentials.get(&credential.0).ok_or_else(invalid_auth)?;
        let token = entry
            .tokens
            .as_ref()
            .filter(|tokens| tokens.id_expires_unix > unix_now())
            .map(|tokens| tokens.id_token.as_str());
        let mut bound = self.bound.lock().map_err(|_| oidc_unavailable())?;
        // Check under the same lock used by release_owner, so cleanup cannot
        // finish before this check and then have activation recreate its binding.
        if ended.is_cancelled() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "session closed or expired",
                AppErrorKind::Session,
            ));
        }
        bound.retain(|binding| binding.owner != owner);
        scope.set_bearer(token);
        bound.push(Binding {
            credential: credential.0.clone(),
            owner: owner.to_owned(),
            scope,
            ended: ended.clone(),
        });
        Ok(())
    }

    pub fn deactivate(&self, credential: &str) {
        self.unbind(|binding| binding.credential == credential);
    }

    pub fn release_owner(&self, owner: &str) {
        self.unbind(|binding| binding.owner == owner);
    }

    fn unbind(&self, matches: impl Fn(&Binding) -> bool) {
        let Ok(mut bound) = self.bound.lock() else {
            return;
        };
        bound.retain(|binding| {
            let remove = matches(binding);
            if remove {
                binding.scope.set_bearer(None);
            }
            !remove
        });
    }

    fn publish(&self, credential: &str) {
        let Ok(mut credentials) = self.credentials.lock() else {
            return;
        };
        self.prune_expired(&mut credentials);
        let token = credentials
            .get(credential)
            .and_then(|entry| entry.tokens.as_ref())
            .filter(|tokens| tokens.id_expires_unix > unix_now())
            .map(|tokens| tokens.id_token.as_str());
        // Registry and bindings remain locked until publication, so logout cannot
        // clear a binding and then have it overwritten with a previously read token.
        let Ok(mut bound) = self.bound.lock() else {
            return;
        };
        bound.retain(|binding| {
            if binding.ended.is_cancelled() {
                binding.scope.set_bearer(None);
                return false;
            }
            true
        });
        for binding in bound
            .iter()
            .filter(|binding| binding.credential == credential)
        {
            binding.scope.set_bearer(token);
        }
    }

    /// Renew only sign-ins with an open session.
    pub async fn refresh_forever(&self) {
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            let mut credentials = self
                .bound
                .lock()
                .map(|bound| {
                    bound
                        .iter()
                        .map(|binding| binding.credential.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            credentials.sort_unstable();
            credentials.dedup();
            for credential in credentials {
                let _ = self.ensure_fresh(&credential).await;
            }
        }
    }

    pub(crate) async fn ensure_fresh(&self, credential: &str) -> Result<(), ApiError> {
        let _refreshing = self.refreshing.lock().await;
        let tokens = {
            let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
            self.prune_expired(&mut credentials);
            credentials
                .get(credential)
                .ok_or_else(invalid_auth)?
                .tokens
                .clone()
                .ok_or_else(invalid_auth)?
        };
        if tokens.id_expires_unix > unix_now() + REFRESH_MARGIN_SECONDS {
            return Ok(());
        }
        let result = self.refresh(&tokens).await;
        {
            let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
            // Expiry or logout during the provider request is final.
            self.prune_expired(&mut credentials);
            let entry = credentials.get_mut(credential).ok_or_else(invalid_auth)?;
            match result {
                Ok(renewed) => entry.tokens = Some(renewed),
                Err(RefreshFailure::Refused) if tokens.id_expires_unix <= unix_now() => {
                    entry.revoked.cancel();
                    credentials.remove(credential);
                    self.deactivate(credential);
                    return Err(invalid_auth());
                }
                Err(_) => {}
            }
        }
        // An expired ID token clears slots, but leaves the binding for retries.
        self.publish(credential);
        Ok(())
    }

    async fn refresh(&self, tokens: &Tokens) -> Result<Tokens, RefreshFailure> {
        let refresh_token = RefreshToken::new(
            tokens
                .refresh_token
                .clone()
                .ok_or(RefreshFailure::Refused)?,
        );
        let metadata = discover_metadata(&self.config.issuer, &self.http)
            .await
            .map_err(|_| RefreshFailure::Transient)?;
        validate_metadata(&self.config.issuer, &metadata, self.allow_http_test)
            .map_err(|_| RefreshFailure::Transient)?;
        let client = self
            .client(&metadata)
            .map_err(|_| RefreshFailure::Transient)?;
        let response = client
            .exchange_refresh_token(&refresh_token)
            .map_err(|_| RefreshFailure::Transient)?
            .request_async(&{
                let http = self.http.clone();
                move |request| send_request(http.clone(), request)
            })
            .await
            .map_err(|error| match error {
                RequestTokenError::ServerResponse(response)
                    if matches!(
                        response.error().as_ref(),
                        "invalid_grant"
                            | "invalid_client"
                            | "unauthorized_client"
                            | "invalid_request"
                            | "invalid_scope"
                            | "unsupported_grant_type"
                    ) =>
                {
                    RefreshFailure::Refused
                }
                _ => RefreshFailure::Transient,
            })?;
        let id_token = response.id_token().ok_or(RefreshFailure::Transient)?;
        let login_nonce = tokens.nonce.clone();
        let claims = id_token
            .claims(
                &self.id_token_verifier(&client),
                |nonce: Option<&Nonce>| match nonce {
                    Some(nonce) if nonce.secret() != &login_nonce => {
                        Err("refreshed ID token nonce differs".to_owned())
                    }
                    _ => Ok(()),
                },
            )
            .map_err(|_| RefreshFailure::Transient)?;
        let (subject, exp) = self
            .accept_claims(claims, &id_token.to_string())
            .ok_or(RefreshFailure::Transient)?;
        if subject != tokens.subject {
            return Err(RefreshFailure::Transient);
        }
        Ok(Tokens {
            id_token: id_token.to_string(),
            id_expires_unix: exp,
            refresh_token: response
                .refresh_token()
                .map(|token| token.secret().clone())
                .or_else(|| tokens.refresh_token.clone()),
            nonce: tokens.nonce.clone(),
            subject,
        })
    }

    pub(super) fn prune_expired(&self, credentials: &mut HashMap<String, Credential>) {
        let now = unix_now();
        let expired = credentials
            .iter()
            .filter(|(_, credential)| credential.expires_unix <= now)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            if let Some(credential) = credentials.remove(&key) {
                credential.revoked.cancel();
            }
            self.deactivate(&key);
        }
    }

    pub fn verify(&self, headers: &HeaderMap) -> Result<AuthSession, ApiError> {
        let token = cookie_value(headers, AUTH_COOKIE).ok_or_else(invalid_auth)?;
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        self.prune_expired(&mut credentials);
        let credential = credentials.get(token).ok_or_else(invalid_auth)?;
        Ok(AuthSession {
            identity: credential.identity.clone(),
            revoked: credential.revoked.clone(),
            credential: OidcCredential(token.to_owned()),
        })
    }

    pub fn logout(&self, headers: &HeaderMap) -> Result<Option<String>, ApiError> {
        let token = cookie_value(headers, AUTH_COOKIE);
        let mut credentials = self.credentials.lock().map_err(|_| oidc_unavailable())?;
        let subject = token
            .and_then(|token| credentials.remove(token))
            .map(|credential| {
                credential.revoked.cancel();
                credential.identity.subject
            });
        if let Some(token) = token {
            self.deactivate(token);
        }
        self.prune_expired(&mut credentials);
        Ok(subject)
    }

    pub fn clear_cookies(&self) -> Result<Vec<HeaderValue>, ApiError> {
        [TRANSACTION_COOKIE, AUTH_COOKIE]
            .into_iter()
            .map(|name| cookie_header(&clear_cookie_header(name)))
            .collect()
    }
}
