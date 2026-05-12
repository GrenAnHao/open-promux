import {
  Copy,
  FolderOpen,
  Loader2,
  RefreshCw,
  Send,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Panel } from "@/components/ui/panel";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useConfig } from "@/hooks/use-config";
import { api } from "@/lib/api";
import type {
  ChatProbeResult,
  RuntimeInfo,
  ServerStatus,
  UpstreamConfig,
  UpstreamHealthSnapshot,
} from "@/lib/types";
import { readUpstreams } from "@/lib/types";
import {
  getModelsEntry,
  patchSelected,
  setModelsEntry,
  subscribe,
  upstreamKey,
} from "@/lib/upstream-probe-cache";
import { cn, formatUptime, resolveDisplayAddress } from "@/lib/utils";

interface DashboardProps {
  status: ServerStatus;
  runtime: RuntimeInfo | null;
  onRefresh: () => void;
}

export function DashboardPage({ status, runtime, onRefresh }: DashboardProps) {
  const { t } = useTranslation();
  const { config } = useConfig();
  const upstreams = readUpstreams(config);
  const [health, setHealth] = useState<UpstreamHealthSnapshot[]>([]);
  const visibleHealth = status.running ? health : [];

  useEffect(() => {
    if (!status.running) {
      return;
    }

    let cancelled = false;
    const poll = async () => {
      let next: UpstreamHealthSnapshot[] = [];
      try {
        next = await api.getUpstreamHealth();
      } catch {
        next = [];
      }
      if (!cancelled) {
        setHealth(next);
      }
    };
    void poll();
    const id = window.setInterval(() => {
      void poll();
    }, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [status.running]);

  return (
    <div className="grid gap-4 p-4 md:grid-cols-3">
      <Panel title={t("dashboard.system")} className="md:col-span-1">
        <dl className="space-y-3">
          <Row
            label={t("dashboard.status")}
            value={status.running ? t("topbar.online") : t("topbar.offline")}
            state={status.running ? "online" : "idle"}
          />
          <BindRow
            label={t("dashboard.bind")}
            address={status.address}
            port={status.port}
            running={status.running}
          />
          <Row
            label={t("dashboard.uptime")}
            value={status.running ? formatUptime(status.uptime_seconds) : "--"}
          />
          <Row label={t("dashboard.version")} value={runtime?.version ?? "--"} />
          <Row label={t("dashboard.platform")} value={runtime?.platform ?? "--"} />
          <Row
            label={t("dashboard.config")}
            value={
              runtime?.config_exists
                ? t("dashboard.configReady")
                : t("dashboard.configMissing")
            }
            state={runtime?.config_exists ? "online" : "warn"}
          />
        </dl>
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Button size="sm" variant="ghost" onClick={onRefresh}>
            <RefreshCw className="size-3.5" />
            {t("common.refresh")}
          </Button>
          <Button
            size="sm"
            variant="link"
            onClick={() =>
              api.openConfigDir().catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="size-3.5" />
            {t("dashboard.configDir")}
          </Button>
          <Button
            size="sm"
            variant="link"
            onClick={() =>
              api.openDebugDir().catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="size-3.5" />
            {t("dashboard.debugDir")}
          </Button>
        </div>
      </Panel>

      <Panel
        title={t("dashboard.upstreams")}
        className="md:col-span-2 min-w-0 flex flex-col"
        bodyClassName="flex-1 min-h-0 flex flex-col"
      >
        {upstreams.length === 0 ? (
          <p className="font-mono text-sm text-ink-500">
            {t("dashboard.noUpstreams")}
          </p>
        ) : (
          <div className="scrollbar-thin -mx-4 -mb-4 flex-1 min-h-0 overflow-x-auto overflow-y-auto px-4 pb-4">
            <UpstreamTable
              upstreams={upstreams}
              health={visibleHealth}
              healthEnabled={config.health.enabled}
              serverRunning={status.running}
            />
          </div>
        )}
      </Panel>

      <EndpointsPanel status={status} className="md:col-span-3 min-w-0" />
    </div>
  );
}

/**
 * Show the proxy's downstream endpoints (the URLs clients should point at)
 * with one-click copy. Base URL is derived from the live `ServerStatus`
 * via `resolveDisplayAddress` so wildcard binds (`0.0.0.0` / `::`) collapse
 * to a usable loopback host.
 */
function EndpointsPanel({
  status,
  className,
}: {
  status: ServerStatus;
  className?: string;
}) {
  const { t } = useTranslation();
  const resolved = status.running
    ? resolveDisplayAddress(status.address, status.port)
    : null;
  const baseUrl = resolved ? `http://${resolved.display}` : null;

  const endpoints: { key: string; label: string; path: string }[] = [
    {
      key: "chat",
      label: t("dashboard.endpointChat"),
      path: "/v1/chat/completions",
    },
    {
      key: "responses",
      label: t("dashboard.endpointResponses"),
      path: "/v1/responses",
    },
    {
      key: "messages",
      label: t("dashboard.endpointMessages"),
      path: "/v1/messages",
    },
  ];

  const copy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      toast.success(t("dashboard.endpointCopied", { url }));
    } catch {
      toast.error(t("logs.clipboardUnavailable"));
    }
  };

  return (
    <Panel title={t("dashboard.endpoints")} className={className}>
      {!baseUrl ? (
        <p className="font-mono text-sm text-ink-500">
          {t("dashboard.endpointsOffline")}
        </p>
      ) : (
        <div className="flex flex-col gap-1.5 font-mono text-[12.5px]">
          <EndpointRow
            label={t("dashboard.endpointBaseUrl")}
            value={baseUrl}
            onCopy={() => void copy(baseUrl)}
          />
          {endpoints.map((ep) => {
            const fullUrl = `${baseUrl}${ep.path}`;
            return (
              <EndpointRow
                key={ep.key}
                label={ep.label}
                value={fullUrl}
                onCopy={() => void copy(fullUrl)}
              />
            );
          })}
        </div>
      )}
    </Panel>
  );
}

function EndpointRow({
  label,
  value,
  onCopy,
}: {
  label: string;
  value: string;
  onCopy: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3">
      <span className="w-44 shrink-0 text-[11px] uppercase tracking-[0.18em] text-ink-500">
        {label}
      </span>
      <span
        className="min-w-0 flex-1 truncate text-ink-200"
        title={value}
      >
        {value}
      </span>
      <Button
        size="icon"
        variant="ghost"
        onClick={onCopy}
        aria-label={t("dashboard.endpointCopyAria", { url: value })}
        title={t("dashboard.endpointCopyAria", { url: value })}
      >
        <Copy className="size-3.5" />
      </Button>
    </div>
  );
}

interface UpstreamTableProps {
  upstreams: UpstreamConfig[];
  health: UpstreamHealthSnapshot[];
  healthEnabled: boolean;
  serverRunning: boolean;
}

function UpstreamTable({
  upstreams,
  health,
  healthEnabled,
  serverRunning,
}: UpstreamTableProps) {
  const { t } = useTranslation();

  // Match health snapshots by `url` rather than array index so the badge
  // stays correct even if the user reorders or deletes upstreams while
  // the server is still running with an older config.
  const healthByUrl = new Map<string, UpstreamHealthSnapshot>();
  for (const snapshot of health) {
    healthByUrl.set(snapshot.url, snapshot);
  }

  return (
    <table className="min-w-full border-collapse font-mono text-[12.5px]">
      <thead>
        <tr className="text-left text-ink-500">
          <Th>{t("dashboard.columnName")}</Th>
          <Th>{t("dashboard.columnUrl")}</Th>
          <Th>{t("dashboard.columnFormat")}</Th>
          <Th>{t("dashboard.columnHealth")}</Th>
          <Th>{t("dashboard.model")}</Th>
          <Th className="text-right">{t("dashboard.columnProbe")}</Th>
        </tr>
      </thead>
      <tbody>
        {upstreams.map((upstream) => (
          <UpstreamRow
            key={`${upstream.name ?? "default"}-${upstreamKey(upstream)}-${upstream.url}`}
            upstream={upstream}
            health={healthByUrl.get(upstream.url)}
            healthEnabled={healthEnabled}
            serverRunning={serverRunning}
          />
        ))}
      </tbody>
    </table>
  );
}

/**
 * Single-row view of one upstream, with inline model selector, refresh
 * and probe actions and a compact status slot.
 *
 * Model fetches and probe results live in a module-level cache
 * (`upstream-probe-cache.ts`) so they survive:
 *   * Tab switches (Radix unmounts inactive `TabsContent` panels).
 *   * Parent re-renders that hand us a fresh `upstream` object with the
 *     same underlying values.
 *
 * The first fetch happens lazily on mount only when there is no cached
 * entry for this upstream. Subsequent mounts read straight from the
 * cache. The refresh button is the only way to bypass the cache.
 */
function UpstreamRow({
  upstream,
  health,
  healthEnabled,
  serverRunning,
}: {
  upstream: UpstreamConfig;
  health?: UpstreamHealthSnapshot;
  healthEnabled: boolean;
  serverRunning: boolean;
}) {
  const { t } = useTranslation();
  const key = upstreamKey(upstream);

  // Subscribe to cache updates; `useSyncExternalStore` gives us a stable
  // snapshot per render and avoids the dual-state (local + cache) bug
  // we'd hit with a plain `useState` mirror.
  const entry = useSyncExternalStore(
    subscribe,
    useCallback(() => getModelsEntry(key), [key]),
  );

  const [modelsLoading, setModelsLoading] = useState(false);
  const [probing, setProbing] = useState(false);

  // `upstream` changes identity across re-renders even when its meaningful
  // fields don't, so we close over it via a ref to keep `refreshModels`
  // stable and depend only on `key` (which already encodes every field
  // that affects what the upstream will return).
  const upstreamRef = useRef(upstream);
  useEffect(() => {
    upstreamRef.current = upstream;
  }, [upstream]);

  const refreshModels = useCallback(
    async (force: boolean) => {
      const current = upstreamRef.current;
      if (!current.url) return;
      if (!force && getModelsEntry(key)) return;
      setModelsLoading(true);
      try {
        const result = await api.fetchUpstreamModels(current);
        const prevSelected = getModelsEntry(key)?.selected ?? "";
        const nextSelected =
          prevSelected && result.models.includes(prevSelected)
            ? prevSelected
            : result.models[0] ?? "";
        setModelsEntry(key, {
          models: result.models,
          error: null,
          selected: nextSelected,
          fetchedAt: Date.now(),
        });
      } catch (err) {
        setModelsEntry(key, {
          models: [],
          error: String(err),
          selected: "",
          fetchedAt: Date.now(),
        });
      } finally {
        setModelsLoading(false);
      }
    },
    [key],
  );

  // Fire the first fetch once per key. A ref-guard prevents the effect
  // from re-firing when the cache notifies us back with the new entry.
  const firedForKey = useRef<string | null>(null);
  useEffect(() => {
    if (firedForKey.current === key) return;
    firedForKey.current = key;
    if (!getModelsEntry(key)) {
      void refreshModels(false);
    }
  }, [key, refreshModels]);

  const models = entry?.models ?? null;
  const modelsError = entry?.error ?? null;
  const selected = entry?.selected ?? "";

  const copyRequestModel = async (model: string) => {
    const requestModel = requestModelName(upstream, model);
    try {
      await navigator.clipboard.writeText(requestModel);
      toast.success(t("dashboard.modelCopied", { model: requestModel }));
    } catch {
      toast.error(t("logs.clipboardUnavailable"));
    }
  };

  const runChatProbe = async () => {
    if (!selected) return;
    setProbing(true);
    try {
      const result = await api.chatProbeUpstream({
        upstream: upstreamRef.current,
        model: selected,
      });
      showChatProbeToast(result, t);
    } catch (err) {
      toast.error(t("dashboard.chatProbeFailed", { error: err }));
    } finally {
      setProbing(false);
    }
  };

  return (
    <tr className="text-ink-100">
      <Td className="text-ink-200 whitespace-nowrap overflow-hidden text-ellipsis max-w-[16ch]">
        {upstream.name ? (
          <span className="block truncate" title={upstream.name}>
            {upstream.name}
          </span>
        ) : (
          <span className="text-ink-500">{t("common.none")}</span>
        )}
      </Td>
      <Td
        className="text-ink-300 whitespace-nowrap overflow-hidden text-ellipsis max-w-[28ch]"
        title={upstream.url}
      >
        <span className="block truncate">{upstream.url}</span>
      </Td>
      <Td className="text-ink-400 whitespace-nowrap">{upstream.api_format}</Td>
      <Td className="whitespace-nowrap">
        <HealthBadge
          health={health}
          healthEnabled={healthEnabled}
          serverRunning={serverRunning}
        />
      </Td>
      <Td className="min-w-[160px] whitespace-nowrap">
        <Select
          value={selected}
          onValueChange={(v) => patchSelected(key, v)}
          disabled={
            modelsLoading || !models || models.length === 0 || !upstream.url
          }
        >
          <SelectTrigger className="h-7 text-[12px]">
            <SelectValue
              placeholder={
                modelsLoading
                  ? t("dashboard.modelsLoading")
                  : modelsError
                    ? t("dashboard.modelsErrorShort")
                    : models && models.length === 0
                      ? t("dashboard.modelsEmpty")
                      : t("dashboard.modelsSelectPlaceholder")
              }
            />
          </SelectTrigger>
          <SelectContent>
            {models?.map((id) => {
              const requestModel = requestModelName(upstream, id);
              return (
                <SelectItem
                  key={id}
                  value={id}
                  className="text-[12px]"
                  action={
                    <button
                      type="button"
                      tabIndex={-1}
                      className="flex size-5 items-center justify-center border border-carbon-400 text-ink-400 hover:border-mint-400/70 hover:text-mint-300 focus-visible:border-mint-400 focus-visible:outline-none"
                      title={t("dashboard.copyRequestModel", {
                        model: requestModel,
                      })}
                      aria-label={t("dashboard.copyRequestModel", {
                        model: requestModel,
                      })}
                      onPointerDown={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                      }}
                      onPointerUp={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                      }}
                      onClick={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                        void copyRequestModel(id);
                      }}
                    >
                      <Copy className="size-3" />
                    </button>
                  }
                >
                  {id}
                </SelectItem>
              );
            })}
          </SelectContent>
        </Select>
      </Td>
      <Td>
        <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void refreshModels(true)}
            disabled={modelsLoading}
            title={t("dashboard.modelsRefresh")}
          >
            {modelsLoading ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="size-3.5" />
            )}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={runChatProbe}
            disabled={probing || !selected}
            title={t("dashboard.chatProbe")}
          >
            {probing ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <Send className="size-3.5" />
            )}
          </Button>
          <ModelsResultLine
            modelsError={modelsError}
            modelsCount={models?.length}
          />
        </div>
      </Td>
    </tr>
  );
}

interface ModelsResultLineProps {
  modelsError: string | null;
  modelsCount: number | undefined;
}

function ModelsResultLine({
  modelsError,
  modelsCount,
}: ModelsResultLineProps) {
  const { t } = useTranslation();

  if (modelsError) {
    return (
      <span
        className="ml-1 inline-flex items-center gap-1.5 text-coral-400"
        title={modelsError}
      >
        <span className="led led-error" />
        {t("dashboard.modelsErrorShort")}
      </span>
    );
  }

  if (typeof modelsCount === "number" && modelsCount > 0) {
    return (
      <span className="ml-1 text-ink-500">
        {t("dashboard.modelsCount", { count: modelsCount })}
      </span>
    );
  }

  return null;
}

function HealthBadge({
  health,
  healthEnabled,
  serverRunning,
}: {
  health?: UpstreamHealthSnapshot;
  healthEnabled: boolean;
  serverRunning: boolean;
}) {
  const { t } = useTranslation();
  if (!serverRunning) {
    return <span className="text-ink-600">--</span>;
  }
  if (!healthEnabled) {
    return <StatusBadge state="idle" label={t("dashboard.healthDisabled")} />;
  }
  if (!health?.checked) {
    return <StatusBadge state="idle" label={t("dashboard.healthPending")} />;
  }
  if (health.healthy) {
    return <StatusBadge state="online" label={t("dashboard.healthHealthy")} />;
  }
  return (
    <StatusBadge
      state="error"
      label={t("dashboard.healthUnhealthy", { count: health.failures })}
    />
  );
}

function StatusBadge({
  state,
  label,
}: {
  state: "online" | "idle" | "error";
  label: string;
}) {
  return (
    <span className="inline-flex items-center gap-1.5 text-ink-300">
      <span
        className={cn(
          state === "online" && "led-online",
          state === "idle" && "led-idle",
          state === "error" && "led-error",
        )}
        aria-hidden
      />
      {label}
    </span>
  );
}

function requestModelName(upstream: UpstreamConfig, model: string) {
  const prefix = upstream.name?.trim();
  return prefix ? `${prefix}:${model}` : model;
}

function showChatProbeToast(
  result: ChatProbeResult,
  t: ReturnType<typeof useTranslation>["t"],
) {
  const detail = result.preview ?? result.message?.slice(0, 120) ?? "";
  const message = `${t(
    result.ok ? "dashboard.chatProbeSucceeded" : "dashboard.chatProbeRejected",
    { status: result.status, latency: result.latency_ms },
  )}${detail ? ` · ${detail}` : ""}`;
  if (result.ok) {
    toast.success(message);
  } else {
    toast.error(message);
  }
}

interface RowProps {
  label: string;
  value: string;
  state?: "online" | "idle" | "warn" | "error";
  hint?: string;
}

interface BindRowProps {
  label: string;
  address?: string | null;
  port?: number | null;
  running: boolean;
}

function BindRow({ label, address, port, running }: BindRowProps) {
  if (!running) {
    return <Row label={label} value="--" />;
  }
  const resolved = resolveDisplayAddress(address, port);
  return <Row label={label} value={resolved.display} hint={resolved.hint} />;
}

function Row({ label, value, state, hint }: RowProps) {
  return (
    <div className="flex items-center justify-between gap-3">
      <dt className="data-label">{label}</dt>
      <dd className="flex items-center gap-2">
        {state && (
          <span
            className={cn(
              state === "online" && "led-online",
              state === "idle" && "led-idle",
              state === "warn" && "led-warn",
              state === "error" && "led-error",
            )}
            aria-hidden
          />
        )}
        <span className="data-value" title={hint}>
          {value}
        </span>
      </dd>
    </div>
  );
}

function Th({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <th
      className={cn(
        "border-b border-carbon-500 py-1.5 pr-3 text-left font-normal text-[11px] uppercase tracking-[0.18em] text-ink-500",
        className,
      )}
    >
      {children}
    </th>
  );
}

function Td({
  className,
  children,
  title,
}: {
  className?: string;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <td
      className={cn(
        "border-b border-carbon-700 py-1.5 pr-3 align-middle",
        className,
      )}
      title={title}
    >
      {children}
    </td>
  );
}
