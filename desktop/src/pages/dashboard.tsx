import { Activity, FolderOpen, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Panel } from "@/components/ui/panel";
import { useConfig } from "@/hooks/use-config";
import { api } from "@/lib/api";
import type {
  RuntimeInfo,
  ServerStatus,
  UpstreamConfig,
  UpstreamProbeResult,
} from "@/lib/types";
import { readUpstreams } from "@/lib/types";
import { cn, formatUptime } from "@/lib/utils";

interface DashboardProps {
  status: ServerStatus;
  runtime: RuntimeInfo | null;
  onRefresh: () => void;
}

export function DashboardPage({ status, runtime, onRefresh }: DashboardProps) {
  const { t } = useTranslation();
  const { config, reload } = useConfig();
  const upstreams = readUpstreams(config);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <div className="grid gap-4 p-4 md:grid-cols-3">
      <Panel title={t("dashboard.system")} className="md:col-span-1">
        <dl className="space-y-3">
          <Row
            label={t("dashboard.status")}
            value={status.running ? t("topbar.online") : t("topbar.offline")}
            state={status.running ? "online" : "idle"}
          />
          <Row
            label={t("dashboard.bind")}
            value={
              status.address ? `${status.address}:${status.port ?? "?"}` : "--"
            }
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
            <RefreshCw className="h-3.5 w-3.5" />
            {t("common.refresh")}
          </Button>
          <Button
            size="sm"
            variant="link"
            onClick={() =>
              api.openConfigDir().catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="h-3.5 w-3.5" />
            {t("dashboard.configDir")}
          </Button>
          <Button
            size="sm"
            variant="link"
            onClick={() =>
              api.openDebugDir().catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="h-3.5 w-3.5" />
            {t("dashboard.debugDir")}
          </Button>
        </div>
      </Panel>

      <Panel title={t("dashboard.upstreams")} className="md:col-span-2">
        {upstreams.length === 0 ? (
          <p className="font-mono text-sm text-ink-500">
            {t("dashboard.noUpstreams")}
          </p>
        ) : (
          <UpstreamTable upstreams={upstreams} />
        )}
      </Panel>
    </div>
  );
}

interface UpstreamTableProps {
  upstreams: UpstreamConfig[];
}

function UpstreamTable({ upstreams }: UpstreamTableProps) {
  const { t } = useTranslation();
  const [probing, setProbing] = useState<Record<number, boolean>>({});
  const [probes, setProbes] = useState<Record<number, UpstreamProbeResult>>({});

  const probe = useCallback(
    async (index: number) => {
      const upstream = upstreams[index];
      if (!upstream) return;
      setProbing((prev) => ({ ...prev, [index]: true }));
      try {
        const result = await api.probeUpstream({
          url: upstream.url,
          apiKey: upstream.api_key || null,
          authHeader: upstream.auth_header || null,
        });
        setProbes((prev) => ({ ...prev, [index]: result }));
      } catch (err) {
        toast.error(t("upstreams.probeFailed", { error: err }));
      } finally {
        setProbing((prev) => ({ ...prev, [index]: false }));
      }
    },
    [upstreams, t],
  );

  return (
    <table className="w-full border-collapse font-mono text-[12.5px]">
      <thead>
        <tr className="text-left text-ink-500">
          <Th>{t("dashboard.columnName")}</Th>
          <Th>{t("dashboard.columnUrl")}</Th>
          <Th>{t("dashboard.columnFormat")}</Th>
          <Th>{t("dashboard.columnProbe")}</Th>
        </tr>
      </thead>
      <tbody>
        {upstreams.map((upstream, index) => (
          <tr
            key={`${upstream.name ?? "default"}-${index}`}
            className="text-ink-100"
          >
            <Td className="text-ink-200">
              {upstream.name || (
                <span className="text-ink-500">{t("common.none")}</span>
              )}
            </Td>
            <Td className="text-ink-300">{upstream.url}</Td>
            <Td className="text-ink-400">{upstream.api_format}</Td>
            <Td>
              <ProbeCell
                probing={!!probing[index]}
                result={probes[index]}
                onProbe={() => probe(index)}
                t={t}
              />
            </Td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

interface RowProps {
  label: string;
  value: string;
  state?: "online" | "idle" | "warn" | "error";
}

function Row({ label, value, state }: RowProps) {
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
        <span className="data-value">{value}</span>
      </dd>
    </div>
  );
}

interface ProbeCellProps {
  probing: boolean;
  result?: UpstreamProbeResult;
  onProbe: () => void;
  t: ReturnType<typeof useTranslation>["t"];
}

function ProbeCell({ probing, result, onProbe, t }: ProbeCellProps) {
  if (probing) {
    return (
      <span className="inline-flex items-center gap-1.5 text-ink-400">
        <Activity className="h-3 w-3 animate-pulse" />
        {t("dashboard.probing")}
      </span>
    );
  }
  if (!result) {
    return (
      <button
        type="button"
        onClick={onProbe}
        className="font-mono text-[11px] uppercase tracking-[0.18em] text-mint-300 hover:text-mint-200"
      >
        {t("dashboard.probeRun")}
      </button>
    );
  }
  if (result.ok) {
    return (
      <button
        type="button"
        onClick={onProbe}
        className="inline-flex items-center gap-1.5 text-mint-300 hover:text-mint-200"
      >
        <span className="led led-online" />
        {result.latency_ms} ms
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={onProbe}
      className="inline-flex items-center gap-1.5 text-coral-400 hover:text-coral-400/90"
      title={result.message ?? undefined}
    >
      <span className="led led-error" />
      {result.status === 0 ? t("dashboard.probeError") : `HTTP ${result.status}`}
    </button>
  );
}

function Th({ children }: { children: React.ReactNode }) {
  return (
    <th className="border-b border-carbon-500 py-1.5 pr-3 text-left font-normal text-[11px] uppercase tracking-[0.18em] text-ink-500">
      {children}
    </th>
  );
}

function Td({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <td className={cn("border-b border-carbon-700 py-1.5 pr-3", className)}>
      {children}
    </td>
  );
}
