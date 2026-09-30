use super::UsageProxyConfig;
use crate::config_toml::ConfigToml;
use pretty_assertions::assert_eq;

#[test]
fn usage_proxy_requires_both_fields_at_top_level_and_in_profiles() {
    for prefix in ["[usage_proxy]", "[profiles.work.usage_proxy]"] {
        for fields in [
            "",
            "url = 'https://example.com/quota'",
            "env_key = 'QUOTA_KEY'",
        ] {
            assert!(toml::from_str::<ConfigToml>(&format!("{prefix}\n{fields}")).is_err());
        }
        let config: ConfigToml = toml::from_str(&format!(
            "{prefix}\nurl = 'https://example.com/quota'\nenv_key = 'QUOTA_KEY'"
        ))
        .unwrap();
        let actual = if prefix == "[usage_proxy]" {
            config.usage_proxy
        } else {
            config.profiles["work"].usage_proxy.clone()
        };
        assert_eq!(
            actual,
            Some(UsageProxyConfig {
                url: "https://example.com/quota".into(),
                env_key: "QUOTA_KEY".into(),
            })
        );
    }
}
