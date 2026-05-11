import { Plus, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
import type { Config, LoadBalanceStrategy } from "@/lib/types";

interface AliasRow {
  source: string;
  target: string;
}

export function RoutingPage() {
  const { t } = useTranslation();
  const { config, save, reload, error } = useConfig();
  const [draft, setDraft] = useState<Config>(config);
  const [aliasRows, setAliasRows] = useState<AliasRow[]>([]);

  useEffect(() => {
    setDraft(config);
    setAliasRows(
      Object.entries(config.routing.model_aliases ?? {}).map(
        ([source, target]) => ({ source, target }),
      ),
    );
  }, [config]);

  useEffect(() => {
    if (error) toast.error(error);
  }, [error]);

  const setRouting = <K extends keyof Config["routing"]>(
    key: K,
    value: Config["routing"][K],
  ) =>
    setDraft((prev) => ({
      ...prev,
      routing: { ...prev.routing, [key]: value },
    }));

  const persist = async () => {
    const aliases: Record<string, string> = {};
    for (const row of aliasRows) {
      const source = row.source.trim();
      const target = row.target.trim();
      if (!source || !target) continue;
      aliases[source] = target;
    }
    const next: Config = {
      ...draft,
      routing: { ...draft.routing, model_aliases: aliases },
    };
    try {
      await save(next);
      toast.success(t("routing.saved"));
    } catch (err) {
      toast.error(t("routing.saveFailed", { error: err }));
    }
  };

  return (
    <div className="grid gap-4 p-4 md:grid-cols-2">
      <Panel title={t("routing.titleRouting")}>
        <div className="grid gap-4">
          <Field label={t("routing.loadBalance")}>
            <Select
              value={draft.routing.load_balance}
              onValueChange={(value) =>
                setRouting("load_balance", value as LoadBalanceStrategy)
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="first">first</SelectItem>
                <SelectItem value="round_robin">round_robin</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          <ToggleField
            label={t("routing.autoFailover")}
            description={t("routing.autoFailoverDesc")}
            checked={draft.routing.automatic_failover}
            onChange={(value) => setRouting("automatic_failover", value)}
          />

          <ToggleField
            label={t("routing.exposeAliases")}
            description={t("routing.exposeAliasesDesc")}
            checked={draft.routing.expose_model_aliases}
            onChange={(value) => setRouting("expose_model_aliases", value)}
          />

          <Field label={t("routing.fallbackModel")}>
            <Input
              value={draft.routing.fallback_model ?? ""}
              onChange={(e) =>
                setRouting("fallback_model", e.target.value || null)
              }
              placeholder={t("routing.fallbackPlaceholder")}
            />
          </Field>
        </div>
      </Panel>

      <Panel
        title={t("routing.titleAliases")}
        trailing={
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              setAliasRows((prev) => prev.concat({ source: "", target: "" }))
            }
          >
            <Plus className="h-3.5 w-3.5" />
            {t("routing.addRow")}
          </Button>
        }
      >
        {aliasRows.length === 0 ? (
          <p className="font-mono text-sm text-ink-500">
            {t("routing.aliasEmpty")}
          </p>
        ) : (
          <div className="space-y-2">
            {aliasRows.map((row, index) => (
              <div key={index} className="flex items-center gap-2">
                <Input
                  className="flex-1"
                  value={row.source}
                  placeholder={t("routing.aliasSource")}
                  onChange={(e) =>
                    setAliasRows((prev) =>
                      prev.map((r, i) =>
                        i === index ? { ...r, source: e.target.value } : r,
                      ),
                    )
                  }
                />
                <span className="font-mono text-xs uppercase tracking-[0.18em] text-ink-500">
                  -&gt;
                </span>
                <Input
                  className="flex-1"
                  value={row.target}
                  placeholder={t("routing.aliasTarget")}
                  onChange={(e) =>
                    setAliasRows((prev) =>
                      prev.map((r, i) =>
                        i === index ? { ...r, target: e.target.value } : r,
                      ),
                    )
                  }
                />
                <Button
                  size="icon"
                  variant="danger"
                  aria-label={t("routing.removeAlias")}
                  onClick={() =>
                    setAliasRows((prev) => prev.filter((_, i) => i !== index))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </Panel>

      <div className="flex items-center justify-end gap-2 md:col-span-2">
        <Button variant="ghost" onClick={() => void reload()}>
          {t("common.discard")}
        </Button>
        <Button variant="primary" onClick={() => void persist()}>
          <Save className="h-3.5 w-3.5" />
          {t("routing.saveAction")}
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
