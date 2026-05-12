import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { TrafficSnapshot } from "@/lib/api";

/**
 * In-process ring buffer that converts cumulative `TrafficSnapshot`s into
 * per-sample deltas suitable for charting.
 *
 * The gateway only exposes monotonic counters (atomic `u64`s held in
 * `TrafficStats`), so we compute throughput by diffing consecutive
 * snapshots on the frontend. State only lives as long as the user keeps
 * the Stats tab mounted; clearing stats or restarting the server resets
 * the baseline naturally on the next sample.
 *
 * Style note: the charts deliberately mirror the rest of the terminal UI
 * — single-pixel carbon-500 borders, mono-stack labels and the project's
 * mint / coral / sky / amber accents. cc-switch's recharts setup served
 * as the visual reference but every colour and axis tick is open-promux.
 */

interface SamplePoint {
  /** Absolute wall-clock ms; useful for keying React renders. */
  t: number;
  /** `hh:mm:ss` rendered into X-axis ticks. */
  label: string;
  /** Per-sample-interval deltas. */
  success: number;
  errors: number;
  bytesIn: number;
  bytesOut: number;
}

/** ~2 min of history at 2 s / sample. Plenty for spotting bursts. */
const MAX_POINTS = 60;

type RechartsModule = typeof import("recharts");
type AxisComponents = Pick<RechartsModule, "CartesianGrid" | "XAxis" | "YAxis">;

interface TrafficChartProps {
  snapshot: TrafficSnapshot;
}

export function TrafficChart({ snapshot }: TrafficChartProps) {
  const { t } = useTranslation();
  const [series, setSeries] = useState<SamplePoint[]>([]);
  const [charts, setCharts] = useState<RechartsModule | null>(null);
  const prevRef = useRef<TrafficSnapshot | null>(null);

  useEffect(() => {
    let cancelled = false;
    void import("recharts").then((module) => {
      if (!cancelled) setCharts(module);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const prev = prevRef.current;
    prevRef.current = snapshot;

    // First snapshot establishes the baseline; no delta to record yet.
    if (!prev) return;

    // If counters went down (e.g. user pressed "clear stats" or the
    // server restarted) treat the new value as a fresh baseline. We
    // intentionally don't insert a negative-spike sample.
    const reset =
      snapshot.global.requests_total < prev.global.requests_total ||
      snapshot.global.bytes_in < prev.global.bytes_in;
    if (reset) {
      setSeries([]);
      return;
    }

    const success = Math.max(
      snapshot.global.requests_success - prev.global.requests_success,
      0,
    );
    const errors = Math.max(
      snapshot.global.requests_error - prev.global.requests_error,
      0,
    );
    const bytesIn = Math.max(
      snapshot.global.bytes_in - prev.global.bytes_in,
      0,
    );
    const bytesOut = Math.max(
      snapshot.global.bytes_out - prev.global.bytes_out,
      0,
    );

    const now = Date.now();
    const labelDate = new Date(now);
    const label = `${String(labelDate.getHours()).padStart(2, "0")}:${String(
      labelDate.getMinutes(),
    ).padStart(2, "0")}:${String(labelDate.getSeconds()).padStart(2, "0")}`;

    setSeries((prevSeries) => {
      const next: SamplePoint[] = [
        ...prevSeries,
        { t: now, label, success, errors, bytesIn, bytesOut },
      ];
      while (next.length > MAX_POINTS) next.shift();
      return next;
    });
  }, [snapshot]);

  const empty = series.length < 2;

  return (
    <div className="grid gap-4 md:grid-cols-2">
      <ChartShell
        title={t("stats.chartRateTitle")}
        subtitle={t("stats.chartRateSubtitle")}
      >
        {empty || !charts ? (
          <ChartEmpty hint={t("stats.chartSampling")} />
        ) : (
          <RateChart charts={charts} series={series} t={t} />
        )}
      </ChartShell>

      <ChartShell
        title={t("stats.chartThroughputTitle")}
        subtitle={t("stats.chartThroughputSubtitle")}
      >
        {empty || !charts ? (
          <ChartEmpty hint={t("stats.chartSampling")} />
        ) : (
          <ThroughputChart charts={charts} series={series} t={t} />
        )}
      </ChartShell>
    </div>
  );
}

function RateChart({
  charts,
  series,
  t,
}: {
  charts: RechartsModule;
  series: SamplePoint[];
  t: (key: string) => string;
}) {
  const { Area, AreaChart, ResponsiveContainer, Tooltip } = charts;
  return (
    <ResponsiveContainer width="100%" height="100%">
      <AreaChart data={series} margin={{ top: 6, right: 12, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="op-grad-success" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#5BE7C4" stopOpacity={0.45} />
            <stop offset="100%" stopColor="#5BE7C4" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="op-grad-errors" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#FF6B6B" stopOpacity={0.45} />
            <stop offset="100%" stopColor="#FF6B6B" stopOpacity={0} />
          </linearGradient>
        </defs>
        {axes(t, charts)}
        <Tooltip content={<ChartTooltip valueSuffix=" req" />} />
        <Area
          type="monotone"
          stackId="rate"
          dataKey="success"
          name={t("stats.chartRateSuccess")}
          stroke="#5BE7C4"
          strokeWidth={1.5}
          fill="url(#op-grad-success)"
          isAnimationActive={false}
        />
        <Area
          type="monotone"
          stackId="rate"
          dataKey="errors"
          name={t("stats.chartRateErrors")}
          stroke="#FF6B6B"
          strokeWidth={1.5}
          fill="url(#op-grad-errors)"
          isAnimationActive={false}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}

function ThroughputChart({
  charts,
  series,
  t,
}: {
  charts: RechartsModule;
  series: SamplePoint[];
  t: (key: string) => string;
}) {
  const { Area, AreaChart, ResponsiveContainer, Tooltip } = charts;
  return (
    <ResponsiveContainer width="100%" height="100%">
      <AreaChart data={series} margin={{ top: 6, right: 12, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="op-grad-bin" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#7DD3FC" stopOpacity={0.45} />
            <stop offset="100%" stopColor="#7DD3FC" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="op-grad-bout" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#FFB347" stopOpacity={0.45} />
            <stop offset="100%" stopColor="#FFB347" stopOpacity={0} />
          </linearGradient>
        </defs>
        {axes(t, charts, true)}
        <Tooltip content={<ChartTooltip bytes />} />
        <Area
          type="monotone"
          dataKey="bytesIn"
          name={t("stats.chartBytesIn")}
          stroke="#7DD3FC"
          strokeWidth={1.5}
          fill="url(#op-grad-bin)"
          isAnimationActive={false}
        />
        <Area
          type="monotone"
          dataKey="bytesOut"
          name={t("stats.chartBytesOut")}
          stroke="#FFB347"
          strokeWidth={1.5}
          fill="url(#op-grad-bout)"
          isAnimationActive={false}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}

/**
 * Returns the three cartesian-grid / axes children as a keyed array.
 *
 * Recharts walks `React.Children.toArray(...)` and matches recognised
 * components by their static `displayName`. Wrapping the trio in a
 * fragment hides them from this walk on some versions, so we expose them
 * as a flat keyed array which React inlines for free.
 */
function axes(t: (key: string) => string, components: AxisComponents, bytes = false) {
  const { CartesianGrid, XAxis, YAxis } = components;
  const tickStyle = {
    fill: "#6E7785",
    fontSize: 10,
    fontFamily:
      '"JetBrains Mono", ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace',
  } as const;
  return [
    <CartesianGrid
      key="grid"
      strokeDasharray="2 4"
      stroke="#1F2731"
      vertical={false}
    />,
    <XAxis
      key="x"
      dataKey="label"
      tick={tickStyle}
      axisLine={{ stroke: "#1F2731" }}
      tickLine={false}
      minTickGap={32}
    />,
    <YAxis
      key="y"
      tick={tickStyle}
      axisLine={false}
      tickLine={false}
      width={48}
      tickFormatter={(value: number) =>
        bytes ? formatBytesShort(value) : `${value}`
      }
      label={{
        value: bytes ? t("stats.chartAxisBytes") : t("stats.chartAxisReqs"),
        angle: -90,
        position: "insideLeft",
        offset: 8,
        style: {
          fill: "#6E7785",
          fontSize: 10,
          fontFamily: '"JetBrains Mono", ui-monospace, Consolas, monospace',
        },
      }}
    />,
  ];
}

function ChartShell({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-md border border-carbon-500 bg-carbon-900/60 p-3">
      <div className="mb-2 flex items-baseline justify-between">
        <h3 className="font-mono text-[11px] uppercase tracking-[0.18em] text-ink-200">
          {title}
        </h3>
        <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-ink-500">
          {subtitle}
        </span>
      </div>
      <div className="h-[200px] w-full">{children}</div>
    </div>
  );
}

function ChartEmpty({ hint }: { hint: string }) {
  return (
    <div className="flex h-full items-center justify-center font-mono text-[11px] uppercase tracking-[0.18em] text-ink-500">
      {hint}
    </div>
  );
}

interface TooltipPayload {
  name: string;
  value: number;
  color: string;
  dataKey: string;
}

function ChartTooltip({
  active,
  payload,
  label,
  bytes,
  valueSuffix,
}: {
  active?: boolean;
  payload?: TooltipPayload[];
  label?: string;
  bytes?: boolean;
  valueSuffix?: string;
}) {
  if (!active || !payload || payload.length === 0) return null;
  return (
    <div className="rounded-md border border-carbon-500 bg-carbon-900/95 p-2 font-mono text-[11px] text-ink-200 shadow-lg backdrop-blur">
      <div className="mb-1 text-[10px] uppercase tracking-[0.18em] text-ink-500">
        {label}
      </div>
      {payload.map((entry) => (
        <div
          key={entry.dataKey}
          className="flex items-center gap-2 leading-tight"
        >
          <span
            className="size-1.5 rounded-full"
            style={{ backgroundColor: entry.color }}
            aria-hidden
          />
          <span className="text-ink-300">{entry.name}</span>
          <span className="ml-auto pl-3 text-ink-100">
            {bytes
              ? formatBytesShort(entry.value)
              : `${entry.value}${valueSuffix ?? ""}`}
          </span>
        </div>
      ))}
    </div>
  );
}

function formatBytesShort(value: number): string {
  if (!Number.isFinite(value)) return "--";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  if (value < 1024 * 1024 * 1024)
    return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
