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
            assert_eq!(
                url::Url::parse(&request.url)
                    .unwrap()
                    .query_pairs()
                    .find(|(key, _)| key == "scope +&="),
                Some(("scope +&=".into(), "project+a & b=1".into()))
            );
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
        let mut expected_url = url::Url::parse(
            &usage_url
                .clone()
                .unwrap_or_else(|| format!("{}/v1/usage", server.uri())),
        )
        .unwrap();
        expected_url
            .query_pairs_mut()
            .append_pair("scope +&=", "project+a & b=1");
        Mock::given(method("GET"))
            .and(path(expected_path))
            .and(header("authorization", "Bearer provider-token"))
            .and(header("x-provider", "custom"))
            .and(header("x-signed-url", expected_url.as_str()))
            .and(query_param("scope +&=", "project+a & b=1"))
            .respond_with(
                ResponseTemplate::new(/*s*/ 200)
                    .set_body_json(serde_json::json!({"plan_type": "pro"})),
            )
            .expect(/*r*/ 1)
            .mount(&server)
            .await;
        let mut provider = test_provider(&server.uri()).await;
        provider.query_params = Some([("scope +&=".into(), "project+a & b=1".into())].into());
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
