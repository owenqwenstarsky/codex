use super::*;
use codex_api::AuthHeadersFuture;
use codex_api::AuthProvider;
use codex_api::AuthProviderFuture;
use codex_http_client::OutboundProxyPolicy;
use codex_http_client::Request;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

struct SigningAuth;

impl AuthProvider for SigningAuth {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}

    fn resolve_auth_headers(&self) -> AuthHeadersFuture<'_> {
        Box::pin(async {
            Ok(HeaderMap::from_iter([(
                http::header::AUTHORIZATION,
                HeaderValue::from_static("Bearer provider-token"),
            )]))
        })
    }

    fn apply_auth(&self, mut request: Request) -> AuthProviderFuture<'_> {
        Box::pin(async move {
            assert_eq!(request.method, Method::GET);
            assert!(request.url.contains("scope=project"));
            request.headers.extend(self.resolve_auth_headers().await?);
            request
                .headers
                .insert("x-signed-url", request.url.parse().unwrap());
            Ok(request)
        })
    }
}

#[tokio::test]
async fn provider_usage_preserves_routing_and_applies_async_auth_to_final_request() {
    let server = MockServer::start().await;
    for usage_url in [
        None,
        Some(format!("{}/wham/usage?existing=value", server.uri())),
    ] {
        let expected_path = if usage_url.is_some() {
            "/wham/usage"
        } else {
            "/v1/usage"
        };
        let expected_url = usage_url.as_ref().map_or_else(
            || format!("{}/v1/usage?scope=project", server.uri()),
            |url| format!("{url}&scope=project"),
        );
        Mock::given(method("GET"))
            .and(path(expected_path))
            .and(header("authorization", "Bearer provider-token"))
            .and(header("x-provider", "custom"))
            .and(header("x-signed-url", expected_url))
            .and(query_param("scope", "project"))
            .respond_with(
                ResponseTemplate::new(/*s*/ 200)
                    .set_body_json(serde_json::json!({"plan_type": "pro"})),
            )
            .expect(/*r*/ 1)
            .mount(&server)
            .await;
        let mut provider = test_provider(&server.uri()).await;
        provider.query_params = Some([("scope".into(), "project".into())].into());
        provider
            .headers
            .insert("x-provider", HeaderValue::from_static("custom"));
        Client::get_provider_rate_limits(
            provider,
            Arc::new(SigningAuth),
            usage_url.as_deref(),
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        )
        .await
        .unwrap();
    }
}

async fn test_provider(base_url: &str) -> Provider {
    let provider =
        codex_model_provider::create_model_provider(Default::default(), /*auth_manager*/ None);
    let mut provider = provider.api_provider().await.unwrap();
    provider.base_url = format!("{base_url}/v1/");
    provider
}

#[tokio::test]
async fn provider_usage_rejects_errors_redirects_and_slow_responses() {
    let server = MockServer::start().await;
    let destination = MockServer::start().await;
    for response in [
        ResponseTemplate::new(/*s*/ 401),
        ResponseTemplate::new(/*s*/ 500),
        ResponseTemplate::new(/*s*/ 200).set_body_string("invalid JSON"),
        ResponseTemplate::new(/*s*/ 302)
            .insert_header("Location", format!("{}/usage", destination.uri())),
        ResponseTemplate::new(/*s*/ 200).set_delay(Duration::from_secs(/*secs*/ 11)),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(response)
            .expect(/*r*/ 1)
            .mount(&server)
            .await;
        assert!(
            Client::get_provider_rate_limits(
                test_provider(&server.uri()).await,
                codex_model_provider::unauthenticated_auth_provider(),
                /*usage_url*/ None,
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await
            .is_err()
        );
    }
    assert!(destination.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn usage_proxy_preserves_exact_url_and_decodes_compatible_snapshots() {
    let server = MockServer::start().await;
    let url = format!("{}/quota/?scope=a%2Fb&scope=second", server.uri());
    let payload = serde_json::json!({
        "plan_type": "pro", "credits": {"has_credits": true, "unlimited": false, "balance": "12"},
        "additional_rate_limits": [{"limit_name": "Extra", "metered_feature": "extra", "rate_limit": {
            "allowed": true, "limit_reached": false, "primary_window": {
                "used_percent": 42, "limit_window_seconds": 3600,
                "reset_at": 2_000_000_000, "reset_after_seconds": 120
            }
        }}]
    });
    Mock::given(method("GET"))
        .and(path("/quota/"))
        .and(header("authorization", "Bearer proxy-key"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(&payload))
        .expect(/*r*/ 1)
        .mount(&server)
        .await;
    let expected =
        Client::rate_limit_snapshots_from_payload(serde_json::from_value(payload).unwrap());
    let actual = Client::get_usage_proxy_rate_limits(
        &url,
        "proxy-key",
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
    )
    .await
    .unwrap();
    assert_eq!(actual, expected);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        (
            &requests[0].url[url::Position::BeforePath..],
            requests[0].headers.get("host").unwrap().to_str().unwrap()
        ),
        (
            "/quota/?scope=a%2Fb&scope=second",
            server.address().to_string().as_str()
        ),
    );
    assert!(requests[0].body.is_empty());
}

#[tokio::test]
async fn usage_proxy_rejects_invalid_urls_and_empty_credentials_before_sending() {
    let server = MockServer::start().await;
    for (url, key) in [
        ("/quota".to_owned(), "key"),
        ("file:///quota".to_owned(), "key"),
        ("ftp://example.com/quota".to_owned(), "key"),
        (server.uri(), ""),
        (server.uri(), "  "),
    ] {
        assert!(
            Client::get_usage_proxy_rate_limits(
                &url,
                key,
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await
            .is_err()
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn usage_proxy_rejects_http_errors_malformed_payloads_redirects_and_timeout() {
    let server = MockServer::start().await;
    let destination = MockServer::start().await;
    for response in [
        ResponseTemplate::new(/*s*/ 401),
        ResponseTemplate::new(/*s*/ 500),
        ResponseTemplate::new(/*s*/ 200).set_body_string("invalid JSON"),
        ResponseTemplate::new(/*s*/ 200).set_body_json(serde_json::json!({"rate_limit": {}})),
        ResponseTemplate::new(/*s*/ 302)
            .insert_header("Location", format!("{}/quota", server.uri()))
            .set_body_json(serde_json::json!({"plan_type": "pro"})),
        ResponseTemplate::new(/*s*/ 307).insert_header("Location", destination.uri()),
        ResponseTemplate::new(/*s*/ 200).set_delay(Duration::from_secs(/*secs*/ 11)),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(response)
            .expect(/*r*/ 1)
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        assert!(
            Client::get_usage_proxy_rate_limits(
                &format!("{}/quota", server.uri()),
                "proxy-key",
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(/*secs*/ 11));
    }
    assert!(destination.received_requests().await.unwrap().is_empty());
}
