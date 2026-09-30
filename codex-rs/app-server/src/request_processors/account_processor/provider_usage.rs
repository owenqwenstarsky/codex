//! Informational quota reads do not authorize ChatGPT account actions.

use super::*;

impl AccountRequestProcessor {
    pub(super) async fn get_informational_usage_response(
        &self,
    ) -> Result<GetAccountRateLimitsResponse, JSONRPCErrorError> {
        tokio::time::timeout(Duration::from_secs(/*secs*/ 10), async {
            let response = if let Some(proxy) = &self.config.usage_proxy {
                let key = std::env::var(&proxy.env_key)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        invalid_request("usage proxy credentials are missing or empty")
                    })?;
                BackendClient::get_usage_proxy_rate_limits(
                    &proxy.url,
                    &key,
                    self.config.http_client_factory(),
                )
                .await
                .map_err(|_| internal_error("failed to fetch usage proxy quota"))?
            } else {
                let provider = create_model_provider(
                    self.config.model_provider.clone(),
                    Some(Arc::clone(&self.auth_manager)),
                );
                let api_provider = provider
                    .api_provider()
                    .await
                    .map_err(|_| invalid_request("failed to resolve usage provider"))?;
                let auth = provider
                    .api_auth()
                    .await
                    .map_err(|_| invalid_request("failed to resolve usage provider credentials"))?;
                BackendClient::get_provider_rate_limits(
                    api_provider,
                    auth,
                    self.config
                        .model_provider
                        .usage_url
                        .as_deref()
                        .map(String::as_str),
                    self.config.http_client_factory(),
                )
                .await
                .map_err(|_| internal_error("failed to fetch provider usage"))?
            };
            let rate_limits = response
                .iter()
                .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
                .or_else(|| response.first())
                .cloned()
                .ok_or_else(|| internal_error("informational usage returned no snapshots"))?;
            Ok(GetAccountRateLimitsResponse {
                rate_limits: rate_limits.into(),
                rate_limits_by_limit_id: Some(
                    response
                        .into_iter()
                        .map(|snapshot| {
                            let id = snapshot.limit_id.clone().unwrap_or_else(|| "codex".into());
                            (id, snapshot.into())
                        })
                        .collect(),
                ),
                ordinary_usage_allowed: None,
                rate_limit_reset_credits: None,
                account_id: None,
                rate_limit_upsell: None,
            })
        })
        .await
        .map_err(|_| internal_error("informational usage fetch timed out"))?
    }
}
