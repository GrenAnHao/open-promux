// Thin wrapper around `@tauri-apps/api/core` invoke calls.
//
// Centralised so the rest of the renderer never imports `invoke` directly
// and so we have one place to mock when adding component tests later.

import { invoke } from "@tauri-apps/api/core";

import type {
  Config,
  LogLine,
  RuntimeInfo,
  ServerStatus,
  UpstreamProbeResult,
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

  // ---- logs ----
  getLogsSnapshot: () => invoke<LogLine[]>("get_logs_snapshot"),
  clearLogs: () => invoke<void>("clear_logs"),

  // ---- platform integration ----
  openConfigDir: () => invoke<void>("open_config_dir"),
  openConfigFile: () => invoke<void>("open_config_file"),
  openDebugDir: () => invoke<void>("open_debug_dir"),

  // ---- diagnostics ----
  probeUpstream: (params: {
    url: string;
    apiKey?: string | null;
    authHeader?: string | null;
  }) => invoke<UpstreamProbeResult>("probe_upstream", params),

  // ---- autostart ----
  getAutostartEnabled: () => invoke<boolean>("get_autostart_enabled"),
  setAutostartEnabled: (enabled: boolean) =>
    invoke<void>("set_autostart_enabled", { enabled }),

  // ---- preferences ----
  getPreferences: () => invoke<DesktopPreferences>("get_preferences"),
  savePreferences: (preferences: DesktopPreferences) =>
    invoke<void>("save_preferences", { preferences }),
};

export interface DesktopPreferences {
  language?: string | null;
}
