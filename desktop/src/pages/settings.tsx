import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { FolderOpen, Info, Save } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { LanguageSwitch } from "@/components/language-switch";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NumberInput } from "@/components/ui/number-input";
import { Panel } from "@/components/ui/panel";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useConfig } from "@/hooks/use-config";
import { api } from "@/lib/api";
import type { Config, DebugLogLevel } from "@/lib/types";
import { cn } from "@/lib/utils";

const LOG_LEVELS: DebugLogLevel[] = [
  "trace",
  "debug",
  "info",
  "warn",
  "error",
];

type BindHost = "127.0.0.1" | "0.0.0.0";

function normaliseBindHost(host: string | undefined): BindHost {
  return host === "0.0.0.0" ? "0.0.0.0" : "127.0.0.1";
}

export function SettingsPage() {
  const { t } = useTranslation();
  const { config, save, reload, error } = useConfig();
  const [draft, setDraft] = useState<Config>(config);
  const [autostart, setAutostart] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(false);

  useEffect(() => setDraft(config), [config]);

  useEffect(() => {
    api
      .getAutostartEnabled()
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

  const setRoot = <K extends keyof Config>(key: K, value: Config[K]) =>
    setDraft((prev) => ({ ...prev, [key]: value }));

  const setSection = <S extends "performance" | "health" | "rectifier" | "debug">(
    section: S,
    update: (current: Config[S]) => Config[S],
  ) => setDraft((prev) => ({ ...prev, [section]: update(prev[section]) }));

  const persist = async () => {
    try {
      await save(draft);
      toast.success(t("settings.saved"));
    } catch (err) {
      toast.error(t("settings.saveFailed", { error: err }));
    }
  };

  const toggleAutostart = async (checked: boolean) => {
    setAutostartLoading(true);
    try {
      await api.setAutostartEnabled(checked);
      setAutostart(checked);
    } catch (err) {
      toast.error(t("settings.autostartFailed", { error: err }));
    } finally {
      setAutostartLoading(false);
    }
  };

  return (
    <div className="grid gap-4 p-4 md:grid-cols-2">
      {error ? (
        <p className="font-mono text-sm text-coral-400 md:col-span-2">{error}</p>
      ) : null}
      <Panel title={t("settings.server")}>
        <div className="grid gap-4">
          <BindHostField
            value={normaliseBindHost(draft.host)}
            onChange={(value) => setRoot("host", value)}
          />
          <Field label={t("settings.listenPort")}>
            <NumberInput
              value={draft.port}
              onChange={(n) => setRoot("port", n ?? 8080)}
              placeholder="8080"
            />
          </Field>
          <Field label={t("settings.proxyAuthKey")}>
            <Input
              type="password"
              value={draft.auth_key ?? ""}
              onChange={(e) => setRoot("auth_key", e.target.value || null)}
              placeholder={t("settings.proxyAuthKeyPlaceholder")}
            />
          </Field>
        </div>
      </Panel>

      <Panel title={t("settings.performance")}>
        <div className="grid gap-4">
          <Field label={t("settings.perUpstreamMaxConcurrent")}>
            <NumberInput
              value={draft.performance.upstream_max_concurrent_requests ?? null}
              onChange={(n) =>
                setSection("performance", (current) => ({
                  ...current,
                  upstream_max_concurrent_requests: n,
                }))
              }
              placeholder={t("common.unlimited")}
            />
          </Field>
          <Field label={t("settings.globalRpm")}>
            <NumberInput
              value={draft.performance.global_rpm ?? null}
              onChange={(n) =>
                setSection("performance", (current) => ({
                  ...current,
                  global_rpm: n,
                }))
              }
              placeholder={t("common.unlimited")}
            />
          </Field>
          <Field label={t("settings.globalTpm")}>
            <NumberInput
              value={draft.performance.global_tpm ?? null}
              onChange={(n) =>
                setSection("performance", (current) => ({
                  ...current,
                  global_tpm: n,
                }))
              }
              placeholder={t("common.unlimited")}
            />
          </Field>
        </div>
      </Panel>

      <Panel title={t("settings.health")}>
        <div className="grid gap-4">
          <ToggleField
            label={t("settings.enabled")}
            description={t("settings.healthEnabledDesc")}
            checked={draft.health.enabled}
            onChange={(value) =>
              setSection("health", (current) => ({ ...current, enabled: value }))
            }
          />
          <Field label={t("settings.interval")}>
            <NumberInput
              value={draft.health.interval_millis}
              onChange={(n) =>
                setSection("health", (current) => ({
                  ...current,
                  interval_millis: n ?? 30_000,
                }))
              }
            />
          </Field>
          <Field label={t("settings.unhealthyAfter")}>
            <NumberInput
              value={draft.health.unhealthy_after_failures}
              onChange={(n) =>
                setSection("health", (current) => ({
                  ...current,
                  unhealthy_after_failures: n ?? 3,
                }))
              }
            />
          </Field>
        </div>
      </Panel>

      <Panel title={t("settings.rectifier")}>
        <div className="grid gap-4">
          <ToggleField
            label={t("settings.enabled")}
            description={t("settings.rectifierEnabledDesc")}
            checked={draft.rectifier.enabled}
            onChange={(value) =>
              setSection("rectifier", (current) => ({
                ...current,
                enabled: value,
              }))
            }
          />
          <ToggleField
            label={t("settings.rectifierSignature")}
            description={t("settings.rectifierSignatureDesc")}
            checked={draft.rectifier.thinking_signature}
            onChange={(value) =>
              setSection("rectifier", (current) => ({
                ...current,
                thinking_signature: value,
              }))
            }
          />
          <ToggleField
            label={t("settings.rectifierBudget")}
            description={t("settings.rectifierBudgetDesc")}
            checked={draft.rectifier.thinking_budget}
            onChange={(value) =>
              setSection("rectifier", (current) => ({
                ...current,
                thinking_budget: value,
              }))
            }
          />
        </div>
      </Panel>

      <DebugSection
        value={draft.debug}
        onChange={(update) => setSection("debug", update)}
      />

      <Panel title={t("settings.desktop")}>
        <div className="grid gap-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="font-mono text-[12px] uppercase tracking-[0.18em] text-ink-200">
                {t("settings.language")}
              </p>
              <p className="mt-1 font-mono text-[11.5px] text-ink-500">
                {t("settings.languageDesc")}
              </p>
            </div>
            <LanguageSwitch />
          </div>
          <ToggleField
            label={t("settings.autostart")}
            description={t("settings.autostartDesc")}
            checked={autostart}
            onChange={(value) => void toggleAutostart(value)}
          />
          {autostartLoading && (
            <p className="font-mono text-[11.5px] text-ink-500">
              {t("settings.autostartWorking")}
            </p>
          )}
        </div>
      </Panel>

      <div className="flex items-center justify-end gap-2 md:col-span-2">
        <Button variant="ghost" onClick={() => void reload()}>
          {t("common.discard")}
        </Button>
        <Button variant="primary" onClick={() => void persist()}>
          <Save className="size-3.5" />
          {t("settings.saveAction")}
        </Button>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <Label>{label}</Label>
      <div className="mt-1.5">{children}</div>
    </div>
  );
}

interface BindHostFieldProps {
  value: BindHost;
  onChange: (next: BindHost) => void;
}

function BindHostField({ value, onChange }: BindHostFieldProps) {
  const { t } = useTranslation();
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="min-w-0">
        <p className="font-mono text-[12px] uppercase tracking-[0.18em] text-ink-200">
          {t("settings.bindHost")}
        </p>
        <p className="mt-1 font-mono text-[11.5px] text-ink-500">
          {value === "127.0.0.1"
            ? t("settings.bindHostHintLocal")
            : t("settings.bindHostHintGlobal")}
        </p>
      </div>
      <div className="flex items-center gap-2">
        <SegmentedRadio<BindHost>
          value={value}
          onChange={onChange}
          options={[
            { value: "127.0.0.1", label: t("settings.bindHostLocal") },
            { value: "0.0.0.0", label: t("settings.bindHostGlobal") },
          ]}
        />
        <InfoTip
          ariaLabel={t("settings.bindHostHelpAria")}
          content={
            <div className="space-y-1.5">
              <p>
                <span className="text-mint-300">
                  {t("settings.bindHostLocal")}
                </span>
                {" — "}
                {t("settings.bindHostTooltipLocal")}
              </p>
              <p>
                <span className="text-amber-400">
                  {t("settings.bindHostGlobal")}
                </span>
                {" — "}
                {t("settings.bindHostTooltipGlobal")}
              </p>
            </div>
          }
        />
      </div>
    </div>
  );
}

interface SegmentedRadioProps<T extends string> {
  value: T;
  onChange: (next: T) => void;
  options: { value: T; label: string }[];
}

function SegmentedRadio<T extends string>({
  value,
  onChange,
  options,
}: SegmentedRadioProps<T>) {
  return (
    <div
      role="radiogroup"
      className="inline-flex border border-carbon-500 bg-carbon-800/60 font-mono text-[12px]"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              "px-3 py-1 uppercase tracking-[0.18em] transition-colors",
              "focus:outline-none focus:bg-carbon-700",
              active
                ? "bg-mint-400/15 text-mint-300"
                : "text-ink-400 hover:text-ink-200",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

function InfoTip({
  content,
  ariaLabel,
}: {
  content: ReactNode;
  ariaLabel: string;
}) {
  return (
    <TooltipPrimitive.Provider delayDuration={150}>
      <TooltipPrimitive.Root>
        <TooltipPrimitive.Trigger asChild>
          <button
            type="button"
            aria-label={ariaLabel}
            className="inline-flex size-5 items-center justify-center text-ink-500 hover:text-ink-200 focus:outline-none focus-visible:text-ink-100"
          >
            <Info className="size-4" />
          </button>
        </TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content
            side="left"
            align="center"
            sideOffset={8}
            collisionPadding={12}
            className="z-50 max-w-[320px] border border-carbon-400 bg-carbon-800 px-3 py-2 font-mono text-[11.5px] leading-relaxed text-ink-200 shadow-glow"
          >
            {content}
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    </TooltipPrimitive.Provider>
  );
}

interface ToggleFieldProps {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}

function ToggleField({
  label,
  description,
  checked,
  onChange,
}: ToggleFieldProps) {
  return (
    <div className="flex items-start justify-between gap-4 border-t border-carbon-700 pt-3 first:border-0 first:pt-0">
      <div>
        <p className="font-mono text-[12px] uppercase tracking-[0.18em] text-ink-200">
          {label}
        </p>
        {description && (
          <p className="mt-1 font-mono text-[11.5px] text-ink-500">
            {description}
          </p>
        )}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

type DebugDraft = Config["debug"];

interface DebugSectionProps {
  value: DebugDraft;
  onChange: (update: (current: DebugDraft) => DebugDraft) => void;
}

/**
 * Debug panel: log level + conversation persistence + open-debug-dir.
 *
 * Kept as a dedicated component (rather than inline JSX in SettingsPage)
 * so the parent stays well below the react-doctor component-size budget
 * and so the "needs a tooltip warning" UX stays co-located with its data.
 */
function DebugSection({ value, onChange }: DebugSectionProps) {
  const { t } = useTranslation();

  return (
    <Panel title={t("settings.debug")}>
      <div className="grid gap-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <p className="font-mono text-[12px] uppercase tracking-[0.18em] text-ink-200">
              {t("settings.enabled")}
            </p>
            <p className="mt-1 font-mono text-[11.5px] text-ink-500">
              {t("settings.debugEnabledDesc")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Switch
              checked={value.enabled}
              onCheckedChange={(next) =>
                onChange((current) => ({ ...current, enabled: next }))
              }
            />
            <InfoTip
              ariaLabel={t("settings.debugHelpAria")}
              content={
                <div className="space-y-1.5">
                  <p className="text-amber-400">
                    {t("settings.debugTooltipTitle")}
                  </p>
                  <p>{t("settings.debugTooltipBody")}</p>
                  <p className="text-ink-400">
                    {t("settings.debugTooltipDir")}
                  </p>
                </div>
              }
            />
          </div>
        </div>

        {value.enabled && (
          <>
            <Field label={t("settings.debugLogLevel")}>
              <Select
                value={value.log_level}
                onValueChange={(next) =>
                  onChange((current) => ({
                    ...current,
                    log_level: next as DebugLogLevel,
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {LOG_LEVELS.map((level) => (
                    <SelectItem key={level} value={level}>
                      {t(`settings.debugLogLevel_${level}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="mt-1.5 font-mono text-[11.5px] text-ink-500">
                {t("settings.debugLogLevelDesc")}
              </p>
            </Field>

            <ToggleField
              label={t("settings.debugLogConversations")}
              description={t("settings.debugLogConversationsDesc")}
              checked={value.log_conversations}
              onChange={(next) =>
                onChange((current) => ({
                  ...current,
                  log_conversations: next,
                }))
              }
            />
          </>
        )}

        <div className="flex flex-wrap items-center gap-2 border-t border-carbon-700 pt-3">
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              api.openDebugDir().catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="size-3.5" />
            {t("settings.debugOpenDir")}
          </Button>
          <span className="font-mono text-[11px] text-ink-500">
            {t("settings.debugDirHint")}
          </span>
        </div>
      </div>
    </Panel>
  );
}
