import { Pause, Play, Terminal } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Toaster, toast } from "sonner";

import { LanguageSwitch } from "@/components/language-switch";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useStatus } from "@/hooks/use-status";
import { api } from "@/lib/api";
import type { RuntimeInfo } from "@/lib/types";
import { cn, formatUptime } from "@/lib/utils";
import { DashboardPage } from "@/pages/dashboard";
import { LogsPage } from "@/pages/logs";
import { RoutingPage } from "@/pages/routing";
import { SettingsPage } from "@/pages/settings";
import { UpstreamsPage } from "@/pages/upstreams";

function App() {
  const { t } = useTranslation();
  const { status, refresh } = useStatus();
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .getRuntimeInfo()
      .then(setRuntime)
      .catch((err) => toast.error(t("toast.runtimeInfoFailed", { error: err })));
  }, [t]);

  const start = async () => {
    setBusy(true);
    try {
      await api.startServer();
      toast.success(t("toast.serverStarted"));
      refresh();
    } catch (err) {
      toast.error(t("toast.startFailed", { error: err }));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await api.stopServer();
      toast.success(t("toast.serverStopped"));
      refresh();
    } catch (err) {
      toast.error(t("toast.stopFailed", { error: err }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="relative z-10 flex h-screen flex-col">
      <TopBar
        status={status}
        runtime={runtime}
        busy={busy}
        onStart={start}
        onStop={stop}
      />

      <Tabs defaultValue="dashboard" className="flex flex-1 min-h-0 flex-col">
        <TabsList>
          <TabsTrigger value="dashboard">{t("tabs.dashboard")}</TabsTrigger>
          <TabsTrigger value="upstreams">{t("tabs.upstreams")}</TabsTrigger>
          <TabsTrigger value="routing">{t("tabs.routing")}</TabsTrigger>
          <TabsTrigger value="logs">{t("tabs.logs")}</TabsTrigger>
          <TabsTrigger value="settings">{t("tabs.settings")}</TabsTrigger>
        </TabsList>

        <div className="scrollbar-thin flex-1 min-h-0 overflow-y-auto">
          <TabsContent value="dashboard">
            <DashboardPage status={status} runtime={runtime} onRefresh={refresh} />
          </TabsContent>
          <TabsContent value="upstreams">
            <UpstreamsPage />
          </TabsContent>
          <TabsContent value="routing">
            <RoutingPage />
          </TabsContent>
          <TabsContent value="logs" className="flex flex-1 min-h-0 flex-col">
            <LogsPage />
          </TabsContent>
          <TabsContent value="settings">
            <SettingsPage />
          </TabsContent>
        </div>
      </Tabs>

      <Toaster
        position="bottom-right"
        theme="dark"
        toastOptions={{
          style: {
            background: "#11161D",
            border: "1px solid #2A3340",
            color: "#E5ECF2",
            fontFamily: "JetBrains Mono, ui-monospace, Consolas, monospace",
            borderRadius: 0,
          },
        }}
      />
    </div>
  );
}

interface TopBarProps {
  status: { running: boolean; address?: string | null; port?: number | null; uptime_seconds: number };
  runtime: RuntimeInfo | null;
  busy: boolean;
  onStart: () => void;
  onStop: () => void;
}

function TopBar({ status, runtime, busy, onStart, onStop }: TopBarProps) {
  const { t } = useTranslation();
  const led = status.running ? "led-online" : "led-idle";
  const stateLabel = status.running ? t("topbar.online") : t("topbar.offline");
  const bind = status.running
    ? `${status.address ?? "0.0.0.0"}:${status.port ?? "?"}`
    : "--";
  const uptime = status.running ? formatUptime(status.uptime_seconds) : "--";

  return (
    <header className="flex h-12 items-center justify-between gap-4 border-b border-carbon-500 bg-carbon-900/80 px-4">
      <div className="flex items-center gap-3">
        <Terminal className="h-4 w-4 text-mint-400" />
        <span className="font-mono text-[12px] uppercase tracking-[0.32em] text-ink-100">
          open-promux
        </span>
        {runtime?.version && (
          <span className="font-mono text-[11px] text-ink-500">
            v{runtime.version}
          </span>
        )}
      </div>

      <div className="flex flex-1 items-center justify-center gap-6">
        <Slot label={t("topbar.state")}>
          <span className={cn("led", led)} />
          <span
            className={cn(
              "data-value",
              status.running ? "text-mint-300" : "text-ink-300",
            )}
          >
            {stateLabel}
          </span>
        </Slot>
        <Slot label={t("topbar.bind")}>
          <span className="data-value">{bind}</span>
        </Slot>
        <Slot label={t("topbar.uptime")}>
          <span className="data-value">{uptime}</span>
        </Slot>
      </div>

      <div className="flex items-center gap-2">
        <LanguageSwitch />
        {status.running ? (
          <Button variant="danger" size="sm" disabled={busy} onClick={onStop}>
            <Pause className="h-3.5 w-3.5" />
            {t("topbar.stop")}
          </Button>
        ) : (
          <Button variant="primary" size="sm" disabled={busy} onClick={onStart}>
            <Play className="h-3.5 w-3.5" />
            {t("topbar.start")}
          </Button>
        )}
      </div>
    </header>
  );
}

function Slot({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="data-label">{label}</span>
      <span className="flex items-center gap-1.5">{children}</span>
    </div>
  );
}

export default App;
