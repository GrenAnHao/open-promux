use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_key: Option<String>,
    #[serde(default)]
    pub performance: PerformanceConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub rectifier: RectifierConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamConfig>,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PerformanceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_max_concurrent_requests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_rpm: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_tpm: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub load_balance: LoadBalanceStrategy,
    #[serde(default)]
    pub automatic_failover: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub expose_model_aliases: bool,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_health_interval_millis")]
    pub interval_millis: u64,
    #[serde(default = "default_unhealthy_after_failures")]
    pub unhealthy_after_failures: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RectifierConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub thinking_signature: bool,
    #[serde(default = "default_true")]
    pub thinking_budget: bool,
}

impl Default for RectifierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thinking_signature: true,
            thinking_budget: true,
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_millis: default_health_interval_millis(),
            unhealthy_after_failures: default_unhealthy_after_failures(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoadBalanceStrategy {
    #[default]
    First,
    RoundRobin,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpstreamConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(
        default = "default_auth_header",
        deserialize_with = "deserialize_auth_header"
    )]
    pub auth_header: String,
    #[serde(
        default,
        deserialize_with = "deserialize_upstream_proxy",
        serialize_with = "serialize_upstream_proxy",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy: Option<UpstreamProxyConfig>,
    #[serde(default)]
    pub proxy_type: UpstreamProxyType,
    #[serde(default)]
    pub api_format: UpstreamApiFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamProxyConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl UpstreamProxyConfig {
    /// Render the proxy back into the same string format accepted by
    /// [`UpstreamProxyConfig::parse`] (`host:port` or `host:port:user:pass`).
    pub fn to_config_string(&self) -> String {
        match (self.username.as_deref(), self.password.as_deref()) {
            (Some(user), Some(pass)) => format!("{}:{}:{}:{}", self.host, self.port, user, pass),
            _ => format!("{}:{}", self.host, self.port),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UpstreamProxyType {
    #[default]
    Http,
    Socks,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UpstreamApiFormat {
    #[default]
    ChatCompletions,
    AnthropicMessages,
}

impl UpstreamProxyType {
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks => "socks5",
        }
    }
}

impl<'de> Deserialize<'de> for LoadBalanceStrategy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "first" => Ok(Self::First),
            "round_robin" | "round-robin" | "roundrobin" => Ok(Self::RoundRobin),
            _ => Err(de::Error::custom(
                "load_balance must be first or round_robin",
            )),
        }
    }
}

impl Serialize for LoadBalanceStrategy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::First => "first",
            Self::RoundRobin => "round_robin",
        };
        serializer.serialize_str(value)
    }
}

impl UpstreamProxyConfig {
    fn parse(raw: &str) -> Result<Self, String> {
        let parts = raw.splitn(4, ':').collect::<Vec<_>>();
        if parts.len() != 2 && parts.len() != 4 {
            return Err("proxy must use host:port or host:port:username:password".into());
        }

        let host = parts[0].trim();
        if host.is_empty() {
            return Err("proxy host cannot be empty".into());
        }

        let port = parts[1]
            .trim()
            .parse::<u16>()
            .map_err(|_| "proxy port must be a valid u16".to_string())?;

        let (username, password) = if parts.len() == 4 {
            let username = parts[2].trim();
            let password = parts[3].trim();
            if username.is_empty() && password.is_empty() {
                (None, None)
            } else if username.is_empty() || password.is_empty() {
                return Err("proxy username and password must both be set or both omitted".into());
            } else {
                (Some(username.to_string()), Some(password.to_string()))
            }
        } else {
            (None, None)
        };

        Ok(Self {
            host: host.to_string(),
            port,
            username,
            password,
        })
    }
}

impl<'de> Deserialize<'de> for UpstreamProxyType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "http" => Ok(Self::Http),
            "socket" | "socks" | "socks5" => Ok(Self::Socks),
            _ => Err(de::Error::custom(
                "proxy_type must be http, socket, socks, or socks5",
            )),
        }
    }
}

impl Serialize for UpstreamProxyType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::Http => "http",
            Self::Socks => "socks",
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for UpstreamApiFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "chat_completions" | "chat-completions" | "openai_chat" | "openai-chat" => {
                Ok(Self::ChatCompletions)
            }
            "anthropic_messages" | "anthropic-messages" | "anthropic" | "messages" => {
                Ok(Self::AnthropicMessages)
            }
            _ => Err(de::Error::custom(
                "api_format must be chat_completions or anthropic_messages",
            )),
        }
    }
}

impl Serialize for UpstreamApiFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::ChatCompletions => "chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
        };
        serializer.serialize_str(value)
    }
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

fn deserialize_upstream_proxy<'de, D>(
    deserializer: D,
) -> Result<Option<UpstreamProxyConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => UpstreamProxyConfig::parse(raw)
            .map(Some)
            .map_err(de::Error::custom),
    }
}

fn serialize_upstream_proxy<S>(
    proxy: &Option<UpstreamProxyConfig>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match proxy {
        Some(proxy) => serializer.serialize_str(&proxy.to_config_string()),
        None => serializer.serialize_none(),
    }
}

fn default_port() -> u16 {
    8080
}

fn default_health_interval_millis() -> u64 {
    30_000
}

fn default_unhealthy_after_failures() -> u64 {
    3
}

fn default_true() -> bool {
    true
}

impl Config {
    /// CLI-friendly loader that panics with a helpful message when the file
    /// is missing or invalid.
    pub fn load(path: &Path) -> Self {
        Self::load_path(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Result-returning loader for embedders such as the desktop UI.
    pub fn load_path(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read config file {}: {e}", path.display()))?;
        Self::from_toml_str(&content)
            .map_err(|e| format!("failed to parse config file {}: {e}", path.display()))
    }

    /// Parse a TOML string into a [`Config`].
    pub fn from_toml_str(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Serialise this config back into a TOML string.
    ///
    /// Note: this loses any comments present in the original file. Use the
    /// raw editor in the desktop UI when comment preservation is required.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
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

    #[test]
    fn config_should_parse_proxy_auth_key() {
        let config: Config = toml::from_str(
            r#"
port = 8080
auth_key = "proxy-secret"

[upstream]
url = "http://example.com/v1"
"#,
        )
        .unwrap();

        assert_eq!(config.auth_key.as_deref(), Some("proxy-secret"));
    }

    #[test]
    fn config_should_default_performance_routing_and_rate_limits_to_disabled() {
        let config: Config = toml::from_str(
            r#"
[upstream]
url = "http://example.com/v1"
"#,
        )
        .unwrap();

        assert_eq!(config.performance.upstream_max_concurrent_requests, None);
        assert_eq!(config.performance.global_rpm, None);
        assert_eq!(config.performance.global_tpm, None);
        assert_eq!(config.routing.load_balance, LoadBalanceStrategy::First);
        assert!(!config.routing.automatic_failover);
        assert_eq!(config.routing.fallback_model, None);
        assert!(!config.routing.expose_model_aliases);
        assert!(config.routing.model_aliases.is_empty());
        assert!(!config.health.enabled);
        assert_eq!(config.health.interval_millis, 30_000);
        assert_eq!(config.health.unhealthy_after_failures, 3);
        assert!(config.rectifier.enabled);
        assert!(config.rectifier.thinking_signature);
        assert!(config.rectifier.thinking_budget);
        assert_eq!(
            config.upstream.as_ref().unwrap().max_concurrent_requests,
            None
        );
        assert_eq!(config.upstream.as_ref().unwrap().rpm, None);
        assert_eq!(config.upstream.as_ref().unwrap().tpm, None);
    }

    #[test]
    fn config_should_parse_routing_and_rate_limit_settings() {
        let config: Config = toml::from_str(
            r#"
[performance]
upstream_max_concurrent_requests = 16
global_rpm = 100
global_tpm = 1000

[routing]
load_balance = "round_robin"
automatic_failover = true
fallback_model = "local:qwen3-coder"
expose_model_aliases = true

[routing.model_aliases]
"gpt-5.5" = "local:qwen3-coder"
"gpt-5.4-mini" = "openai:gpt-4.1-mini"

[health]
enabled = true
interval_millis = 1000
unhealthy_after_failures = 2

[rectifier]
enabled = false
thinking_signature = false
thinking_budget = false

[upstream]
url = "http://example.com/v1"
max_concurrent_requests = 4
rpm = 10
tpm = 200
"#,
        )
        .unwrap();

        assert_eq!(
            config.performance.upstream_max_concurrent_requests,
            Some(16)
        );
        assert_eq!(config.performance.global_rpm, Some(100));
        assert_eq!(config.performance.global_tpm, Some(1000));
        assert_eq!(config.routing.load_balance, LoadBalanceStrategy::RoundRobin);
        assert!(config.routing.automatic_failover);
        assert_eq!(
            config.routing.fallback_model.as_deref(),
            Some("local:qwen3-coder")
        );
        assert!(config.routing.expose_model_aliases);
        assert_eq!(
            config
                .routing
                .model_aliases
                .get("gpt-5.5")
                .map(String::as_str),
            Some("local:qwen3-coder")
        );
        assert_eq!(
            config
                .routing
                .model_aliases
                .get("gpt-5.4-mini")
                .map(String::as_str),
            Some("openai:gpt-4.1-mini")
        );
        assert!(config.health.enabled);
        assert_eq!(config.health.interval_millis, 1000);
        assert_eq!(config.health.unhealthy_after_failures, 2);
        assert!(!config.rectifier.enabled);
        assert!(!config.rectifier.thinking_signature);
        assert!(!config.rectifier.thinking_budget);
        assert_eq!(
            config.upstream.as_ref().unwrap().max_concurrent_requests,
            Some(4)
        );
        assert_eq!(config.upstream.as_ref().unwrap().rpm, Some(10));
        assert_eq!(config.upstream.as_ref().unwrap().tpm, Some(200));
    }

    #[test]
    fn config_should_parse_upstream_http_proxy_without_auth() {
        let config: Config = toml::from_str(
            r#"
[upstream]
url = "http://example.com/v1"
proxy = "127.0.0.1:7890"
proxy_type = "http"
"#,
        )
        .unwrap();

        let upstream = config.upstream.as_ref().unwrap();
        let proxy = upstream.proxy.as_ref().unwrap();

        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 7890);
        assert!(proxy.username.is_none());
        assert!(proxy.password.is_none());
        assert_eq!(upstream.proxy_type, UpstreamProxyType::Http);
    }

    #[test]
    fn config_should_parse_upstream_socket_proxy_with_auth() {
        let config: Config = toml::from_str(
            r#"
[upstream]
url = "http://example.com/v1"
proxy = "proxy.example.com:1080:user:pass"
proxy_type = "socket"
"#,
        )
        .unwrap();

        let upstream = config.upstream.as_ref().unwrap();
        let proxy = upstream.proxy.as_ref().unwrap();

        assert_eq!(proxy.host, "proxy.example.com");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username.as_deref(), Some("user"));
        assert_eq!(proxy.password.as_deref(), Some("pass"));
        assert_eq!(upstream.proxy_type, UpstreamProxyType::Socks);
    }

    #[test]
    fn config_should_round_trip_through_toml_for_desktop_editor() {
        let original: Config = toml::from_str(
            r#"
port = 9090
auth_key = "secret"

[performance]
upstream_max_concurrent_requests = 32
global_rpm = 600
global_tpm = 120000

[routing]
load_balance = "round_robin"
automatic_failover = true
fallback_model = "openai:gpt-4.1-mini"
expose_model_aliases = true

[routing.model_aliases]
"gpt-5.5" = "openai:gpt-4.1"

[health]
enabled = true
interval_millis = 5000
unhealthy_after_failures = 2

[rectifier]
enabled = false
thinking_signature = false
thinking_budget = false

[[upstreams]]
name = "openai"
url = "https://api.openai.com/v1"
api_key = "sk-test"
proxy = "127.0.0.1:1080:user:pass"
proxy_type = "socks"
api_format = "anthropic_messages"
max_concurrent_requests = 16
rpm = 100
tpm = 50000
"#,
        )
        .unwrap();

        let toml_text = original.to_toml_string().expect("serialize config");
        let parsed: Config = Config::from_toml_str(&toml_text).expect("reparse config");

        assert_eq!(parsed.port, 9090);
        assert_eq!(parsed.auth_key.as_deref(), Some("secret"));
        assert_eq!(
            parsed.performance.upstream_max_concurrent_requests,
            Some(32)
        );
        assert_eq!(parsed.routing.load_balance, LoadBalanceStrategy::RoundRobin);
        assert!(parsed.routing.automatic_failover);
        assert_eq!(
            parsed.routing.fallback_model.as_deref(),
            Some("openai:gpt-4.1-mini")
        );
        assert!(parsed.routing.expose_model_aliases);
        assert_eq!(
            parsed
                .routing
                .model_aliases
                .get("gpt-5.5")
                .map(String::as_str),
            Some("openai:gpt-4.1")
        );
        assert!(parsed.health.enabled);
        assert_eq!(parsed.health.interval_millis, 5000);
        assert_eq!(parsed.health.unhealthy_after_failures, 2);
        assert!(!parsed.rectifier.enabled);
        assert!(!parsed.rectifier.thinking_signature);
        assert!(!parsed.rectifier.thinking_budget);

        let upstream = &parsed.upstreams[0];
        assert_eq!(upstream.name.as_deref(), Some("openai"));
        assert_eq!(upstream.url, "https://api.openai.com/v1");
        assert_eq!(upstream.api_key, "sk-test");
        let proxy = upstream.proxy.as_ref().expect("proxy preserved");
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username.as_deref(), Some("user"));
        assert_eq!(proxy.password.as_deref(), Some("pass"));
        assert_eq!(upstream.proxy_type, UpstreamProxyType::Socks);
        assert_eq!(upstream.api_format, UpstreamApiFormat::AnthropicMessages);
        assert_eq!(upstream.max_concurrent_requests, Some(16));
        assert_eq!(upstream.rpm, Some(100));
        assert_eq!(upstream.tpm, Some(50000));
    }
}
