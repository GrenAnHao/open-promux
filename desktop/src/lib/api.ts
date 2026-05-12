// Thin wrapper around `@tauri-apps/api/core` invoke calls.
//
// Centralised so the rest of the renderer never imports `invoke` directly
// and so we have one place to mock when adding component tests later.

import { invoke } from "@tauri-apps/api/core";

import type {
  ChatProbeResult,
  Config,
  FetchedModels,
  LogLine,
  RuntimeInfo,
  ServerStatus,
  UpstreamConfig,
  UpstreamHealthSnapshot,
} from "./types";

export const api = {
  // ---- runtime / config ----
  getRuntimeInfo: () => invoke<RuntimeInfo>("get_runtime_info"),
  setConfigPath: (path: string) =>
    invoke<RuntimeInfo>("set_config_path", { path }),
  loadConfig: () => invoke<Config>("load_config"),
  loadConfigText: () => invoke<string>("load_config_text"),
  saveConfig: (config: Config) => invoke<void>("save_config", { config }),
  saveConfigText: (content: string) =>
    invoke<void>("save_config_text", { content }),

  // ---- server lifecycle ----
  startServer: () => invoke<ServerStatus>("start_server"),
  stopServer: () => invoke<void>("stop_server"),
  getStatus: () => invoke<ServerStatus>("get_status"),
  getUpstreamHealth: () => invoke<UpstreamHealthSnapshot[]>("get_upstream_health"),

  // ---- logs ----
  getLogsSnapshot: () => invoke<LogLine[]>("get_logs_snapshot"),
  clearLogs: () => invoke<void>("clear_logs"),

  // ---- platform integration ----
  openConfigDir: () => invoke<void>("open_config_dir"),
  openConfigFile: () => invoke<void>("open_config_file"),
  openDebugDir: () => invoke<void>("open_debug_dir"),

  // ---- diagnostics ----
  fetchUpstreamModels: (upstream: UpstreamConfig) =>
    invoke<FetchedModels>("fetch_upstream_models", { upstream }),
  chatProbeUpstream: (params: {
    upstream: UpstreamConfig;
    model: string;
    prompt?: string | null;
  }) => invoke<ChatProbeResult>("chat_probe_upstream", params),

  // ---- autostart ----
  getAutostartEnabled: () => invoke<boolean>("get_autostart_enabled"),
  setAutostartEnabled: (enabled: boolean) =>
    invoke<void>("set_autostart_enabled", { enabled }),

  // ---- preferences ----
  getPreferences: () => invoke<DesktopPreferences>("get_preferences"),
  savePreferences: (preferences: DesktopPreferences) =>
    invoke<void>("save_preferences", { preferences }),

  // ---- traffic stats ----
  getTrafficStats: () => invoke<TrafficSnapshot>("get_traffic_stats"),
  clearTrafficStats: () => invoke<void>("clear_traffic_stats"),
};

interface CounterSnapshot {
  requests_total: number;
  requests_success: number;
  requests_error: number;
  bytes_in: number;
  bytes_out: number;
  latency_ms_avg: number;
  latency_ms_max: number;
}

interface UpstreamCounters {
  upstream: string;
  counters: CounterSnapshot;
}

interface ModelCounters {
  upstream: string;
  model: string;
  counters: CounterSnapshot;
}

export interface TrafficSnapshot {
  uptime_seconds: number;
  global: CounterSnapshot;
  upstreams: UpstreamCounters[];
  models: ModelCounters[];
}

interface DesktopPreferences {
  language?: string | null;
}
