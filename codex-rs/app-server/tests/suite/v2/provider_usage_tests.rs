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

#[tokio::test]
async fn usage_proxy_takes_precedence_and_isolates_host_credentials_without_fallback() -> Result<()>
{
    let proxy = MockServer::start().await;
    let inference = MockServer::start().await;
    let destination = MockServer::start().await;
    let expected: GetAccountRateLimitsResponse = serde_json::from_value(json!({
        "rateLimits": {"limitId": "codex", "limitName": null, "normalModelSlug": null,
            "primary": null, "secondary": null, "credits": null, "individualLimit": null,
            "spendControlReached": null, "planType": "pro", "rateLimitReachedType": null},
        "rateLimitsByLimitId": {"codex": {"limitId": "codex", "limitName": null, "normalModelSlug": null,
            "primary": null, "secondary": null, "credits": null, "individualLimit": null,
            "spendControlReached": null, "planType": "pro", "rateLimitReachedType": null}},
        "ordinaryUsageAllowed": null, "rateLimitResetCredits": null,
        "accountId": null, "rateLimitUpsell": null,
    }))?;
    for (provider, supports_usage, stored_chatgpt) in [
        ("openai", false, false),
        ("openai", false, true),
        ("custom", false, true),
        ("custom", true, true),
    ] {
        for key in [Some("proxy-key"), None, Some(""), Some("  ")] {
            proxy.reset().await;
            Mock::given(method("GET"))
                .and(path("/quota"))
                .and(header("authorization", "Bearer proxy-key"))
                .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(json!({
                    "plan_type": "pro", "ordinary_usage_allowed": true,
                    "account_id": "proxy-account", "rate_limit_reset_credits": {"available_count": 5},
                    "rate_limit_upsell": {"title": "Buy credits"},
                })))
                .expect(if key == Some("proxy-key") { 1 } else { 0 }).mount(&proxy).await;
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
            let inference_url = inference.uri();
            let proxy_url = proxy.uri();
            std::fs::write(
                codex_home.path().join("config.toml"),
                format!(
                    r#"
model_provider = "{provider}"
chatgpt_base_url = "{inference_url}"
[usage_proxy]
url = "{proxy_url}/quota?scope=a%2Fb&scope=second"
env_key = "USAGE_PROXY_TEST_KEY"
[model_providers.custom]
name = "Custom"
base_url = "{inference_url}/v1"
usage_url = "{inference_url}/provider-usage"
env_key = "INFERENCE_PROXY_TEST_KEY"
supports_usage = {supports_usage}
http_headers = {{ "x-provider" = "secret" }}
query_params = {{ "provider_scope" = "secret" }}
"#
                ),
            )?;
            let mut app = TestAppServer::builder()
                .with_codex_home(codex_home.path())
                .with_env_overrides(&[
                    ("USAGE_PROXY_TEST_KEY", key),
                    (
                        "INFERENCE_PROXY_TEST_KEY",
                        if supports_usage {
                            Some("inference-secret")
                        } else {
                            None
                        },
                    ),
                ])
                .build_initialized_with_timeout(std::time::Duration::from_secs(/*secs*/ 25))
                .await?;
            let id = app.send_get_account_rate_limits_request().await?;
            if key != Some("proxy-key") {
                let error: JSONRPCError = app
                    .read_stream_until_error_message(RequestId::Integer(id))
                    .await?;
                assert_eq!(error.error.code, -32600);
                assert!(proxy.received_requests().await.unwrap().is_empty());
                continue;
            }
            let actual: GetAccountRateLimitsResponse = app.read_response(id).await?;
            assert_eq!(actual, expected);
            let requests = proxy.received_requests().await.unwrap();
            assert_eq!(
                (
                    &requests[0].url[url::Position::BeforePath..],
                    requests[0].headers.get("host").unwrap().to_str().unwrap()
                ),
                (
                    "/quota?scope=a%2Fb&scope=second",
                    proxy.address().to_string().as_str()
                ),
            );
            assert!(requests[0].body.is_empty());
            for name in ["x-provider", "chatgpt-account-id", "openai-organization"] {
                assert!(!requests[0].headers.contains_key(name));
            }
            // A failure after a successful refresh still never reads provider/account usage or billing.
            let mut failures = vec![
                ResponseTemplate::new(/*s*/ 401),
                ResponseTemplate::new(/*s*/ 500),
                ResponseTemplate::new(/*s*/ 200).set_body_string("invalid JSON"),
                ResponseTemplate::new(/*s*/ 302).insert_header("Location", destination.uri()),
            ];
            if !stored_chatgpt {
                failures.push(
                    ResponseTemplate::new(/*s*/ 200)
                        .set_delay(std::time::Duration::from_secs(/*secs*/ 11)),
                );
            }
            for response in failures {
                proxy.reset().await;
                Mock::given(method("GET"))
                    .respond_with(response)
                    .expect(/*r*/ 1)
                    .mount(&proxy)
                    .await;
                let id = app.send_get_account_rate_limits_request().await?;
                let error: JSONRPCError = app
                    .read_stream_until_error_message(RequestId::Integer(id))
                    .await?;
                assert_eq!(error.error.code, -32603);
            }
        }
    }
    // Account initialization may perform bootstrap reads; proxy quota never triggers account/provider usage or billing.
    let fallback_or_billing_paths: Vec<_> = inference
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|request| request.url.path().to_owned())
        .filter(|path| path.contains("usage") || path.contains("rate-limit-reset-credits"))
        .collect();
    assert_eq!(fallback_or_billing_paths, Vec::<String>::new());
    assert!(destination.received_requests().await.unwrap().is_empty());
    Ok(())
}
