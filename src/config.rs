use serde::{Deserialize, Deserializer, de};
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub auth_key: Option<String>,
    #[serde(default)]
    pub performance: PerformanceConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub upstream: Option<UpstreamConfig>,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PerformanceConfig {
    #[serde(default)]
    pub upstream_max_concurrent_requests: Option<usize>,
    #[serde(default)]
    pub global_rpm: Option<u64>,
    #[serde(default)]
    pub global_tpm: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub load_balance: LoadBalanceStrategy,
    #[serde(default)]
    pub automatic_failover: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_health_interval_millis")]
    pub interval_millis: u64,
    #[serde(default = "default_unhealthy_after_failures")]
    pub unhealthy_after_failures: u64,
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
    #[serde(default, deserialize_with = "deserialize_upstream_proxy")]
    pub proxy: Option<UpstreamProxyConfig>,
    #[serde(default)]
    pub proxy_type: UpstreamProxyType,
    #[serde(default)]
    pub max_concurrent_requests: Option<usize>,
    #[serde(default)]
    pub rpm: Option<u64>,
    #[serde(default)]
    pub tpm: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamProxyConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamProxyType {
    Http,
    Socks,
}

impl Default for UpstreamProxyType {
    fn default() -> Self {
        Self::Http
    }
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

fn default_port() -> u16 {
    8080
}

fn default_health_interval_millis() -> u64 {
    30_000
}

fn default_unhealthy_after_failures() -> u64 {
    3
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
        assert!(!config.health.enabled);
        assert_eq!(config.health.interval_millis, 30_000);
        assert_eq!(config.health.unhealthy_after_failures, 3);
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

[health]
enabled = true
interval_millis = 1000
unhealthy_after_failures = 2

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
        assert!(config.health.enabled);
        assert_eq!(config.health.interval_millis, 1000);
        assert_eq!(config.health.unhealthy_after_failures, 2);
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
}
