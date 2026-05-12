// Mirrors the Rust types exposed by the open-promux library and desktop
// commands. Keep this file in lockstep with `src/config.rs` and
// `desktop/src-tauri/src/commands.rs`.

export type LoadBalanceStrategy = "first" | "round_robin";
export type UpstreamProxyType = "http" | "socks";
export type UpstreamApiFormat =
  | "chat_completions"
  | "anthropic_messages"
  | "responses";

interface PerformanceConfig {
  upstream_max_concurrent_requests?: number | null;
  global_rpm?: number | null;
  global_tpm?: number | null;
}

interface RoutingConfig {
  load_balance: LoadBalanceStrategy;
  automatic_failover: boolean;
  fallback_model?: string | null;
  expose_model_aliases: boolean;
  model_aliases: Record<string, string>;
}

interface HealthConfig {
  enabled: boolean;
  interval_millis: number;
  unhealthy_after_failures: number;
}

interface RectifierConfig {
  enabled: boolean;
  thinking_signature: boolean;
  thinking_budget: boolean;
}

export type DebugLogLevel = "trace" | "debug" | "info" | "warn" | "error";

/**
 * Mirrors `DebugConfig` in `src/config.rs`.
 *
 * - `enabled`: master switch for the Debug panel; when off the gateway
 *   behaves exactly like before and nothing in this struct is honoured.
 * - `log_level`: tracing filter applied on process start (picks up next
 *   desktop launch); `RUST_LOG` still wins when set.
 * - `log_conversations`: when true, every inbound request body is written
 *   to `./debug/conversation-*.json`. Upstream errors are still captured
 *   separately via the existing `upstream-error-*.json` dumps.
 */
interface DebugConfig {
  enabled: boolean;
  log_level: DebugLogLevel;
  log_conversations: boolean;
}

export interface UpstreamConfig {
  name?: string | null;
  url: string;
  api_key: string;
  auth_header: string;
  proxy?: string | null;
  proxy_type: UpstreamProxyType;
  api_format: UpstreamApiFormat;
  max_concurrent_requests?: number | null;
  rpm?: number | null;
  tpm?: number | null;
}

export interface Config {
  /**
   * Listen interface. `127.0.0.1` keeps the proxy local-only; `0.0.0.0`
   * accepts traffic from the LAN. Mirrors `Config.host` in `src/config.rs`.
   */
  host: string;
  port: number;
  auth_key?: string | null;
  performance: PerformanceConfig;
  routing: RoutingConfig;
  health: HealthConfig;
  rectifier: RectifierConfig;
  debug: DebugConfig;
  upstream?: UpstreamConfig | null;
  upstreams: UpstreamConfig[];
}

export interface RuntimeInfo {
  version: string;
  config_path: string;
  config_exists: boolean;
  platform: string;
}

export interface ServerStatus {
  running: boolean;
  address?: string | null;
  port?: number | null;
  uptime_seconds: number;
}

export interface UpstreamHealthSnapshot {
  index: number;
  name?: string | null;
  url: string;
  api_format: UpstreamApiFormat;
  checked: boolean;
  healthy: boolean;
  failures: number;
}

export interface FetchedModels {
  models: string[];
  latency_ms: number;
}

export interface ChatProbeResult {
  ok: boolean;
  status: number;
  latency_ms: number;
  model: string;
  preview?: string | null;
  message?: string | null;
}

export interface LogLine {
  seq: number;
  ts_millis: number;
  level: "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR";
  target: string;
  message: string;
}

/**
 * Build an empty config used when the user has never written `config.toml`.
 * Mirrors the defaults in `RectifierConfig`/`HealthConfig`/etc.
 */
export function emptyConfig(): Config {
  return {
    host: "127.0.0.1",
    port: 8080,
    auth_key: null,
    performance: {},
    routing: {
      load_balance: "first",
      automatic_failover: false,
      expose_model_aliases: false,
      model_aliases: {},
    },
    health: {
      enabled: false,
      interval_millis: 30_000,
      unhealthy_after_failures: 3,
    },
    rectifier: {
      enabled: true,
      thinking_signature: true,
      thinking_budget: true,
    },
    debug: {
      enabled: false,
      log_level: "info",
      log_conversations: false,
    },
    upstream: null,
    upstreams: [],
  };
}

/**
 * `Config.upstreams` may be empty when only the legacy `[upstream]`
 * single-value form is configured. The desktop UI always shows the table
 * shape, so flatten both into a single array for editing.
 *
 * Defensive against missing fields: a freshly-loaded config that has no
 * upstreams may serialize `upstreams` as absent rather than `[]`.
 */
export function readUpstreams(config: Config): UpstreamConfig[] {
  const list = config.upstreams ?? [];
  if (list.length > 0) return list;
  if (config.upstream) return [config.upstream];
  return [];
}

export function emptyUpstream(): UpstreamConfig {
  return {
    name: null,
    url: "",
    api_key: "",
    auth_header: "Authorization",
    proxy: null,
    proxy_type: "http",
    api_format: "chat_completions",
    max_concurrent_requests: null,
    rpm: null,
    tpm: null,
  };
}

/**
 * Coerce a raw value coming from the Tauri command into a fully-populated
 * Config. Serde may omit empty arrays / maps, so we top up the missing
 * sections from {@link emptyConfig}. Callers should always run their
 * loaded values through this function before passing them to components.
 */
export function normalizeConfig(raw: Partial<Config> | null | undefined): Config {
  const base = emptyConfig();
  if (!raw) return base;
  return {
    ...base,
    ...raw,
    host: raw.host ?? base.host,
    performance: { ...base.performance, ...(raw.performance ?? {}) },
    routing: {
      ...base.routing,
      ...(raw.routing ?? {}),
      model_aliases: raw.routing?.model_aliases ?? {},
    },
    health: { ...base.health, ...(raw.health ?? {}) },
    rectifier: { ...base.rectifier, ...(raw.rectifier ?? {}) },
    debug: { ...base.debug, ...(raw.debug ?? {}) },
    upstreams: raw.upstreams ?? [],
    upstream: raw.upstream ?? null,
  };
}
