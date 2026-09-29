use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::RequestId;
use codex_config::types::AuthCredentialsStoreMode;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn custom_usage_respects_provider_authentication_and_returns_display_data_only() -> Result<()>
{
    let server = MockServer::start().await;
    let window = json!({"used_percent": 42, "limit_window_seconds": 3600,
        "reset_at": 2_000_000_000, "reset_after_seconds": 120});
    let limit = json!({"allowed": true, "limit_reached": false,
        "primary_window": window, "secondary_window": window});
    Mock::given(method("GET"))
        .and(path("/v1/usage"))
        .and(header("authorization", "Bearer custom-key"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(json!({
            "plan_type": "pro", "rate_limit": limit,
            "credits": {"has_credits": true, "unlimited": false, "balance": "12"},
            "additional_rate_limits": [{"limit_name": "Extra model", "metered_feature": "extra", "rate_limit": limit}],
            "account_id": "custom-account", "rate_limit_upsell": {"title": "Buy credits"},
            "rate_limit_reset_credits": {"available_count": 5},
        })))
        .expect(/*r*/ 2).mount(&server).await;
    let display_window =
        json!({"usedPercent": 42, "windowDurationMins": 60, "resetsAt": 2_000_000_000});
    let main = json!({"limitId": "codex", "limitName": null, "normalModelSlug": null,
        "primary": display_window, "secondary": display_window,
        "credits": {"hasCredits": true, "unlimited": false, "balance": "12"},
        "individualLimit": null, "spendControlReached": null,
        "planType": "pro", "rateLimitReachedType": null});
    let extra = json!({"limitId": "extra", "limitName": "Extra model", "normalModelSlug": null,
        "primary": display_window, "secondary": display_window, "credits": null,
        "individualLimit": null, "spendControlReached": null,
        "planType": "pro", "rateLimitReachedType": null});
    let expected: GetAccountRateLimitsResponse = serde_json::from_value(json!({
        "ordinaryUsageAllowed": null, "rateLimits": main,
        "rateLimitsByLimitId": {"codex": main, "extra": extra},
        "rateLimitResetCredits": null, "accountId": null, "rateLimitUpsell": null,
    }))?;
    for (api_key, stored_chatgpt) in [
        (Some("custom-key"), false),
        (Some("custom-key"), true),
        (None, false),
    ] {
        let codex_home = TempDir::new()?;
        if stored_chatgpt {
            write_chatgpt_auth(
                codex_home.path(),
                ChatGptAuthFixture::new("chatgpt-token")
                    .account_id("chatgpt-account")
                    .plan_type("pro"),
                AuthCredentialsStoreMode::File,
            )?;
        }
        let base_url = server.uri();
        std::fs::write(
            codex_home.path().join("config.toml"),
            format!(
                r#"
model_provider = "custom"
[model_providers.custom]
name = "Custom"
base_url = "{base_url}/v1"
env_key = "CUSTOM_USAGE_TEST_KEY"
supports_usage = true
"#
            ),
        )?;
        let mut app = TestAppServer::builder()
            .with_codex_home(codex_home.path())
            .with_env_overrides(&[("CUSTOM_USAGE_TEST_KEY", api_key)])
            .build_initialized()
            .await?;
        let id = app.send_get_account_rate_limits_request().await?;
        if api_key.is_some() {
            let actual: GetAccountRateLimitsResponse = app.read_response(id).await?;
            assert_eq!(actual, expected);
        } else {
            let error: JSONRPCError = app
                .read_stream_until_error_message(RequestId::Integer(id))
                .await?;
            assert_eq!(error.error.code, -32600);
        }
    }
    // No reset-credit or other ChatGPT billing endpoints are needed.
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    Ok(())
}
