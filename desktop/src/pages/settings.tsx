import { Save } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { LanguageSwitch } from "@/components/language-switch";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NumberInput } from "@/components/ui/number-input";
import { Panel } from "@/components/ui/panel";
import { Switch } from "@/components/ui/switch";
import { useConfig } from "@/hooks/use-config";
import { api } from "@/lib/api";
import type { Config } from "@/lib/types";

export function SettingsPage() {
  const { t } = useTranslation();
  const { config, save, reload, error } = useConfig();
  const [draft, setDraft] = useState<Config>(config);
  const [autostart, setAutostart] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(false);

  useEffect(() => setDraft(config), [config]);

  useEffect(() => {
    if (error) toast.error(error);
  }, [error]);

  useEffect(() => {
    api
      .getAutostartEnabled()
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

  const setRoot = <K extends keyof Config>(key: K, value: Config[K]) =>
    setDraft((prev) => ({ ...prev, [key]: value }));

  const setSection = <S extends "performance" | "health" | "rectifier">(
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
      <Panel title={t("settings.server")}>
        <div className="grid gap-4">
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
          <Save className="h-3.5 w-3.5" />
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
