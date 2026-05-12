import { Eraser, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { TrafficChart } from "@/components/stats/traffic-chart";
import { Button } from "@/components/ui/button";
import { Panel } from "@/components/ui/panel";
import { api, type TrafficSnapshot } from "@/lib/api";
import { formatUptime } from "@/lib/utils";

const REFRESH_INTERVAL_MS = 2000;

const EMPTY_SNAPSHOT: TrafficSnapshot = {
  uptime_seconds: 0,
  global: {
    requests_total: 0,
    requests_success: 0,
    requests_error: 0,
    bytes_in: 0,
    bytes_out: 0,
    latency_ms_avg: 0,
    latency_ms_max: 0,
  },
  upstreams: [],
  models: [],
};

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function StatsPage() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<TrafficSnapshot>(EMPTY_SNAPSHOT);

  const reload = useCallback(async () => {
    try {
      const next = await api.getTrafficStats();
      setSnapshot(next);
    } catch (err) {
      toast.error(t("stats.loadFailed", { error: err }));
    }
  }, [t]);

  useEffect(() => {
    void reload();
    const id = window.setInterval(() => {
      void reload();
    }, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [reload]);

  const clear = async () => {
    try {
      await api.clearTrafficStats();
      await reload();
      toast.success(t("stats.cleared"));
    } catch (err) {
      toast.error(t("stats.clearFailed", { error: err }));
    }
  };

  const empty =
    snapshot.global.requests_total === 0 &&
    snapshot.upstreams.length === 0 &&
    snapshot.models.length === 0;

  return (
    <div className="space-y-4 p-4">
      <Panel
        title={t("stats.titleGlobal")}
        trailing={
          <>
            <Button size="sm" variant="ghost" onClick={() => void reload()}>
              <RefreshCw className="size-3.5" />
              {t("stats.refresh")}
            </Button>
            <Button size="sm" variant="danger" onClick={() => void clear()}>
              <Eraser className="size-3.5" />
              {t("stats.clear")}
            </Button>
          </>
        }
      >
        <div className="grid grid-cols-2 gap-x-6 gap-y-3 md:grid-cols-4">
          <Metric
            label={t("stats.metricUptime")}
            value={formatUptime(snapshot.uptime_seconds)}
          />
          <Metric
            label={t("stats.metricRequests")}
            value={snapshot.global.requests_total.toLocaleString()}
          />
          <Metric
            label={t("stats.metricSuccess")}
            value={snapshot.global.requests_success.toLocaleString()}
            tone="ok"
          />
          <Metric
            label={t("stats.metricError")}
            value={snapshot.global.requests_error.toLocaleString()}
            tone={snapshot.global.requests_error > 0 ? "error" : undefined}
          />
          <Metric
            label={t("stats.metricBytesIn")}
            value={formatBytes(snapshot.global.bytes_in)}
          />
          <Metric
            label={t("stats.metricBytesOut")}
            value={formatBytes(snapshot.global.bytes_out)}
          />
          <Metric
            label={t("stats.metricLatencyAvg")}
            value={`${snapshot.global.latency_ms_avg} ms`}
          />
          <Metric
            label={t("stats.metricLatencyMax")}
            value={`${snapshot.global.latency_ms_max} ms`}
          />
        </div>
      </Panel>

      <Panel title={t("stats.titleChart")}>
        <TrafficChart snapshot={snapshot} />
      </Panel>

      <Panel title={t("stats.titleUpstreams")}>
        {snapshot.upstreams.length === 0 ? (
          <EmptyHint />
        ) : (
          <UpstreamTable rows={snapshot.upstreams} />
        )}
      </Panel>

      <Panel title={t("stats.titleModels")}>
        {snapshot.models.length === 0 ? (
          <EmptyHint />
        ) : (
          <ModelTable rows={snapshot.models} />
        )}
      </Panel>

      {empty && (
        <p className="px-1 font-mono text-[11px] uppercase tracking-[0.18em] text-ink-500">
          {t("stats.empty")}
        </p>
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "ok" | "error";
}) {
  const toneClass =
    tone === "ok"
      ? "text-mint-300"
      : tone === "error"
        ? "text-coral-400"
        : "text-ink-100";
  return (
    <div className="flex flex-col gap-0.5">
      <span className="data-label">{label}</span>
      <span className={`font-mono text-base ${toneClass}`}>{value}</span>
    </div>
  );
}

function EmptyHint() {
  const { t } = useTranslation();
  return (
    <p className="p-3 font-mono text-[12px] text-ink-500">{t("stats.empty")}</p>
  );
}

interface UpstreamRow {
  upstream: string;
  counters: TrafficSnapshot["global"];
}

function UpstreamTable({ rows }: { rows: UpstreamRow[] }) {
  const { t } = useTranslation();
  return (
    <div className="overflow-x-auto">
      <table className="min-w-full font-mono text-[12px]">
        <thead className="bg-carbon-700/40 text-[11px] uppercase tracking-[0.18em] text-ink-500">
          <tr>
            <Th>{t("stats.columnUpstream")}</Th>
            <Th align="right">{t("stats.columnRequests")}</Th>
            <Th align="right">{t("stats.columnSuccess")}</Th>
            <Th align="right">{t("stats.columnError")}</Th>
            <Th align="right">{t("stats.columnBytesIn")}</Th>
            <Th align="right">{t("stats.columnBytesOut")}</Th>
            <Th align="right">{t("stats.columnLatencyAvg")}</Th>
            <Th align="right">{t("stats.columnLatencyMax")}</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr
              key={row.upstream}
              className="border-t border-carbon-500/60 hover:bg-carbon-700/30"
            >
              <Td>{row.upstream}</Td>
              <Td align="right">{row.counters.requests_total.toLocaleString()}</Td>
              <Td align="right" tone="ok">
                {row.counters.requests_success.toLocaleString()}
              </Td>
              <Td
                align="right"
                tone={row.counters.requests_error > 0 ? "error" : undefined}
              >
                {row.counters.requests_error.toLocaleString()}
              </Td>
              <Td align="right">{formatBytes(row.counters.bytes_in)}</Td>
              <Td align="right">{formatBytes(row.counters.bytes_out)}</Td>
              <Td align="right">{row.counters.latency_ms_avg}</Td>
              <Td align="right">{row.counters.latency_ms_max}</Td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

interface ModelRow {
  upstream: string;
  model: string;
  counters: TrafficSnapshot["global"];
}

function ModelTable({ rows }: { rows: ModelRow[] }) {
  const { t } = useTranslation();
  return (
    <div className="overflow-x-auto">
      <table className="min-w-full font-mono text-[12px]">
        <thead className="bg-carbon-700/40 text-[11px] uppercase tracking-[0.18em] text-ink-500">
          <tr>
            <Th>{t("stats.columnUpstream")}</Th>
            <Th>{t("stats.columnModel")}</Th>
            <Th align="right">{t("stats.columnRequests")}</Th>
            <Th align="right">{t("stats.columnSuccess")}</Th>
            <Th align="right">{t("stats.columnError")}</Th>
            <Th align="right">{t("stats.columnBytesIn")}</Th>
            <Th align="right">{t("stats.columnBytesOut")}</Th>
            <Th align="right">{t("stats.columnLatencyAvg")}</Th>
            <Th align="right">{t("stats.columnLatencyMax")}</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr
              key={`${row.upstream}::${row.model}`}
              className="border-t border-carbon-500/60 hover:bg-carbon-700/30"
            >
              <Td>{row.upstream}</Td>
              <Td>{row.model}</Td>
              <Td align="right">{row.counters.requests_total.toLocaleString()}</Td>
              <Td align="right" tone="ok">
                {row.counters.requests_success.toLocaleString()}
              </Td>
              <Td
                align="right"
                tone={row.counters.requests_error > 0 ? "error" : undefined}
              >
                {row.counters.requests_error.toLocaleString()}
              </Td>
              <Td align="right">{formatBytes(row.counters.bytes_in)}</Td>
              <Td align="right">{formatBytes(row.counters.bytes_out)}</Td>
              <Td align="right">{row.counters.latency_ms_avg}</Td>
              <Td align="right">{row.counters.latency_ms_max}</Td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Th({
  children,
  align,
}: {
  children: React.ReactNode;
  align?: "left" | "right";
}) {
  return (
    <th
      className={`px-3 py-2 ${align === "right" ? "text-right" : "text-left"}`}
    >
      {children}
    </th>
  );
}

function Td({
  children,
  align,
  tone,
}: {
  children: React.ReactNode;
  align?: "left" | "right";
  tone?: "ok" | "error";
}) {
  const toneClass =
    tone === "ok"
      ? "text-mint-300"
      : tone === "error"
        ? "text-coral-400"
        : "text-ink-200";
  return (
    <td
      className={`px-3 py-1.5 ${align === "right" ? "text-right" : "text-left"} ${toneClass}`}
    >
      {children}
    </td>
  );
}
