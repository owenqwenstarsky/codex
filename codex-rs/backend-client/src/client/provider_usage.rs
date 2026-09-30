//! Read-only usage statistics from an explicitly enabled model provider.

use super::Client;
use crate::types::RateLimitStatusPayload;
use anyhow::Result;
use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_http_client::HttpClientFactory;
use codex_http_client::HttpTransport;
use codex_http_client::ReqwestTransport;
use codex_protocol::protocol::RateLimitSnapshot;
use http::Method;
use http::header::CONTENT_TYPE;
use std::time::Duration;

impl Client {
    /// Reads Codex-compatible usage with provider credentials, without following redirects.
    pub async fn get_provider_rate_limits(
        mut provider: Provider,
        auth: SharedAuthProvider,
        usage_url: Option<&str>,
        http_client_factory: HttpClientFactory,
    ) -> Result<Vec<RateLimitSnapshot>> {
        let client = Self::new_without_redirects(&provider.base_url, http_client_factory);
        let query_params = provider.query_params.take();
        let mut request = provider.build_request(Method::GET, "usage");
        let mut url = url::Url::parse(usage_url.unwrap_or(&request.url))?;
        if !matches!(url.scheme(), "http" | "https") {
            anyhow::bail!("usage endpoint must be an absolute HTTP(S) URL");
        }
        if let Some(params) = query_params {
            url.query_pairs_mut().extend_pairs(params);
        }
        request.url = url.into();
        request.timeout = Some(Duration::from_secs(/*secs*/ 10));
        let request = auth.apply_auth(request).await?;
        let url = request.url.clone();
        let response = ReqwestTransport::from_route_aware_client_pool(client.http.clone())
            .execute(request)
            .await?;
        let content_type = response
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let payload: RateLimitStatusPayload =
            client.decode_json(&url, content_type, std::str::from_utf8(&response.body)?)?;
        Ok(Self::rate_limit_snapshots_from_payload(payload))
    }
}

#[cfg(test)]
#[path = "provider_usage_tests.rs"]
mod tests;
