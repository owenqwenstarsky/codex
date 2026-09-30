//! Informational usage from an opted-in model provider or a dedicated quota proxy.

use super::Client;
use crate::types::RateLimitStatusPayload;
use anyhow::Result;
use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_http_client::HttpTransport;
use codex_http_client::Request;
use codex_http_client::ReqwestTransport;
use codex_http_client::RouteAwareClientPool;
use codex_protocol::protocol::RateLimitSnapshot;
use http::HeaderValue;
use http::Method;
use http::header::AUTHORIZATION;
use http::header::CONTENT_TYPE;
use std::time::Duration;

impl Client {
    /// Reads Codex-compatible usage with provider credentials, without following redirects.
    pub async fn get_provider_rate_limits(
        provider: Provider,
        auth: SharedAuthProvider,
        usage_url: Option<&str>,
        http_client_factory: HttpClientFactory,
    ) -> Result<Vec<RateLimitSnapshot>> {
        let client = Self::new_without_redirects(&provider.base_url, http_client_factory);
        let mut request = provider.build_request(Method::GET, "usage");
        if let Some(usage_url) = usage_url {
            let mut url = url::Url::parse(usage_url)?;
            if !matches!(url.scheme(), "http" | "https") {
                anyhow::bail!("usage_url must be an absolute HTTP(S) URL");
            }
            if let Some(params) = &provider.query_params {
                url.query_pairs_mut().extend_pairs(params);
            }
            request.url = url.into();
        }
        request.timeout = Some(Duration::from_secs(/*secs*/ 10));
        let request = auth.apply_auth(request).await?;
        client.get_usage_snapshots(request).await
    }

    /// Reads a complete quota URL using only a dedicated bearer credential.
    pub async fn get_usage_proxy_rate_limits(
        usage_url: &str,
        key: &str,
        http_client_factory: HttpClientFactory,
    ) -> Result<Vec<RateLimitSnapshot>> {
        let url = url::Url::parse(usage_url)?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            anyhow::bail!("usage proxy URL must be an absolute HTTP(S) URL");
        }
        if key.trim().is_empty() {
            anyhow::bail!("usage proxy credentials are empty");
        }
        let client = Self::with_http(
            usage_url.to_owned(),
            RouteAwareClientPool::new_without_redirects_or_request_logging(
                http_client_factory,
                ClientRouteClass::Api,
            ),
        );
        let mut request = Request::new(Method::GET, usage_url.to_owned());
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))?;
        authorization.set_sensitive(true);
        request.headers.insert(AUTHORIZATION, authorization);
        request.timeout = Some(Duration::from_secs(/*secs*/ 10));
        tokio::time::timeout(
            Duration::from_secs(/*secs*/ 10),
            client.get_usage_snapshots(request),
        )
        .await?
    }

    async fn get_usage_snapshots(&self, request: Request) -> Result<Vec<RateLimitSnapshot>> {
        let url = request.url.clone();
        let response = ReqwestTransport::from_route_aware_client_pool(self.http.clone())
            .execute(request)
            .await?;
        let content_type = response
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let payload: RateLimitStatusPayload =
            self.decode_json(&url, content_type, std::str::from_utf8(&response.body)?)?;
        Ok(Self::rate_limit_snapshots_from_payload(payload))
    }
}

#[cfg(test)]
#[path = "provider_usage_tests.rs"]
mod tests;
