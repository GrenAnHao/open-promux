use serde::{Deserialize, Deserializer};
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub upstream: Option<UpstreamConfig>,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(
        default = "default_auth_header",
        deserialize_with = "deserialize_auth_header"
    )]
    pub auth_header: String,
}

fn default_auth_header() -> String {
    "Authorization".into()
}

fn deserialize_auth_header<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_auth_header);

    if value.trim().is_empty() {
        Ok(default_auth_header())
    } else {
        Ok(value)
    }
}

fn default_port() -> u16 {
    8080
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read config file {}: {e}", path.display()));
        toml::from_str(&content)
            .unwrap_or_else(|e| panic!("failed to parse config file {}: {e}", path.display()))
    }

    pub fn configured_upstreams(&self) -> Vec<&UpstreamConfig> {
        if self.upstreams.is_empty() {
            self.upstream.iter().collect()
        } else {
            self.upstreams.iter().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_should_default_missing_auth_header_to_openai_authorization() {
        let config: Config = toml::from_str(
            r#"
[upstream]
url = "http://example.com/v1"
api_key = "test-key"
"#,
        )
        .unwrap();

        assert_eq!(
            config.upstream.as_ref().unwrap().auth_header,
            "Authorization"
        );
    }

    #[test]
    fn config_should_default_empty_auth_header_to_openai_authorization() {
        let config: Config = toml::from_str(
            r#"
[upstream]
url = "http://example.com/v1"
api_key = "test-key"
auth_header = ""
"#,
        )
        .unwrap();

        assert_eq!(
            config.upstream.as_ref().unwrap().auth_header,
            "Authorization"
        );
    }

    #[test]
    fn config_should_parse_multiple_upstreams_without_legacy_upstream() {
        let parsed = toml::from_str::<Config>(
            r#"
[[upstreams]]
name = "upstream-a"
url = "http://upstream-a.example/v1"
api_key = "key-a"

[[upstreams]]
name = "upstream-b"
url = "http://upstream-b.example/v1"
api_key = "key-b"
auth_header = "api-key"
"#,
        );

        let config = parsed.unwrap();

        assert_eq!(config.configured_upstreams().len(), 2);
        assert_eq!(config.upstreams[0].name.as_deref(), Some("upstream-a"));
    }
}
