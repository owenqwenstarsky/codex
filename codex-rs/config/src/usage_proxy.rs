use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Informational quota endpoint, independent of the inference provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct UsageProxyConfig {
    /// Absolute HTTP(S) URL to read with a bodyless GET.
    pub url: String,
    /// Environment variable containing the bearer token on the app-server host.
    pub env_key: String,
}

#[cfg(test)]
#[path = "usage_proxy_tests.rs"]
mod tests;
