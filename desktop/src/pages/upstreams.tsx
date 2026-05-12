import { Pencil, Plus, Save, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { useConfig } from "@/hooks/use-config";
import {
  type Config,
  type UpstreamApiFormat,
  type UpstreamConfig,
  type UpstreamProxyType,
  emptyUpstream,
  readUpstreams,
} from "@/lib/types";
import { upstreamKey } from "@/lib/upstream-probe-cache";

export function UpstreamsPage() {
  const { t } = useTranslation();
  const { config, loading, error, save, reload } = useConfig();
  const [editing, setEditing] = useState<{
    index: number | null;
    draft: UpstreamConfig;
  } | null>(null);

  // Always render via the merged-list helper, but keep persistence aware of
  // whether the user is using the legacy `[upstream]` form or the table form.
  const upstreams = readUpstreams(config);
  const columnLabels = [
    t("upstreams.columnName"),
    t("upstreams.columnUrl"),
    t("upstreams.columnAuthHeader"),
    t("upstreams.columnFormat"),
    t("upstreams.columnProxy"),
    t("upstreams.columnLimits"),
    "",
  ];

  const beginAdd = () =>
    setEditing({ index: null, draft: emptyUpstream() });
  const beginEdit = (index: number) =>
    setEditing({ index, draft: { ...upstreams[index] } });

  const persist = async (next: Config) => {
    try {
      await save(next);
      toast.success(t("upstreams.saved"));
    } catch (err) {
      toast.error(t("upstreams.saveFailed", { error: err }));
    }
  };

  const remove = async (index: number) => {
    const next: Config = {
      ...config,
      upstream: null,
      upstreams: upstreams.filter((_, i) => i !== index),
    };
    await persist(next);
  };

  const submit = async () => {
    if (!editing) return;
    const draft = sanitize(editing.draft);
    if (!draft.url) {
      toast.error(t("upstreams.urlRequired"));
      return;
    }

    let nextList: UpstreamConfig[];
    if (editing.index === null) {
      nextList = [...upstreams, draft];
    } else {
      nextList = upstreams.map((u, i) => (i === editing.index ? draft : u));
    }

    // Persist purely as the multi-upstream array; collapse the legacy
    // `[upstream]` field so it never duplicates with `[[upstreams]]`.
    const next: Config = {
      ...config,
      upstream: null,
      upstreams: nextList,
    };
    await persist(next);
    setEditing(null);
  };

  return (
    <div className="flex flex-1 min-h-0 flex-col p-4">
      <Panel
        title={t("upstreams.title")}
        className="flex flex-1 min-h-0 flex-col min-w-0"
        bodyClassName="flex-1 min-h-0 flex flex-col"
        trailing={
          <>
            <Button size="sm" variant="ghost" onClick={() => void reload()}>
              {t("common.reload")}
            </Button>
            <Button size="sm" variant="primary" onClick={beginAdd}>
              <Plus className="size-3.5" />
              {t("common.add")}
            </Button>
          </>
        }
      >
        {error ? (
          <p className="mb-3 font-mono text-sm text-coral-400">{error}</p>
        ) : null}
        {loading ? (
          <p className="font-mono text-sm text-ink-500">{t("common.loading")}</p>
        ) : upstreams.length === 0 ? (
          <p className="font-mono text-sm text-ink-500">
            {t("upstreams.empty", { addLabel: t("common.add") })}
          </p>
        ) : (
          <div className="scrollbar-thin -mx-4 -mb-4 flex-1 min-h-0 overflow-x-auto overflow-y-auto px-4 pb-4">
            <table className="min-w-full border-collapse font-mono text-[12.5px]">
            <thead>
              <tr>
                {columnLabels.map((label) => (
                  <th
                    key={label || "actions"}
                    className="border-b border-carbon-500 py-1.5 pr-3 text-left text-[11px] font-normal uppercase tracking-[0.18em] text-ink-500"
                  >
                    {label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {upstreams.map((upstream, index) => (
                <tr key={upstreamRowKey(upstream)} className="text-ink-100">
                  <td className="border-b border-carbon-700 py-1.5 pr-3">
                    {upstream.name || (
                      <span className="text-ink-500">{t("common.none")}</span>
                    )}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-3 text-ink-300">
                    {upstream.url}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-3 text-ink-300">
                    {upstream.auth_header}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-3 text-ink-400">
                    {upstream.api_format}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-3 text-ink-400">
                    {upstream.proxy ? `${upstream.proxy_type}://${upstream.proxy}` : "--"}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-3 text-ink-400">
                    {upstream.rpm ?? "-"} / {upstream.tpm ?? "-"}
                  </td>
                  <td className="border-b border-carbon-700 py-1.5 pr-1 text-right">
                    <div className="flex justify-end gap-1">
                      <Button
                        size="icon"
                        variant="ghost"
                        onClick={() => beginEdit(index)}
                        aria-label={t("common.edit")}
                      >
                        <Pencil className="size-3.5" />
                      </Button>
                      <Button
                        size="icon"
                        variant="danger"
                        onClick={() => void remove(index)}
                        aria-label={t("common.delete")}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          </div>
        )}
      </Panel>

      <Dialog
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) setEditing(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {editing?.index === null
                ? t("upstreams.dialogAdd")
                : t("upstreams.dialogEdit")}
            </DialogTitle>
          </DialogHeader>
          {editing && (
            <UpstreamForm
              draft={editing.draft}
              onChange={(next) =>
                setEditing((prev) => prev && { ...prev, draft: next })
              }
            />
          )}
          <DialogFooter>
            <DialogClose asChild>
              <Button variant="ghost">{t("common.cancel")}</Button>
            </DialogClose>
            <Button variant="primary" onClick={() => void submit()}>
              <Save className="size-3.5" />
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

interface UpstreamFormProps {
  draft: UpstreamConfig;
  onChange: (next: UpstreamConfig) => void;
}

function UpstreamForm({ draft, onChange }: UpstreamFormProps) {
  const { t } = useTranslation();
  const set = <K extends keyof UpstreamConfig>(key: K, value: UpstreamConfig[K]) =>
    onChange({ ...draft, [key]: value });

  return (
    <div className="grid gap-4 p-4 md:grid-cols-2">
      <Field label={t("upstreams.fieldName")}>
        <Input
          value={draft.name ?? ""}
          onChange={(e) => set("name", e.target.value || null)}
          placeholder={t("upstreams.placeholderName")}
        />
      </Field>
      <Field label={t("upstreams.fieldApiFormat")}>
        <Select
          value={draft.api_format}
          onValueChange={(value) =>
            set("api_format", value as UpstreamApiFormat)
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="chat_completions">chat_completions</SelectItem>
            <SelectItem value="anthropic_messages">anthropic_messages</SelectItem>
            <SelectItem value="responses">responses</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      <Field label={t("upstreams.fieldBaseUrl")} full>
        <Input
          value={draft.url}
          onChange={(e) => set("url", e.target.value)}
          placeholder={t("upstreams.placeholderUrl")}
        />
      </Field>
      <Field label={t("upstreams.fieldApiKey")}>
        <Input
          type="password"
          value={draft.api_key}
          onChange={(e) => set("api_key", e.target.value)}
          placeholder={t("upstreams.placeholderApiKey")}
        />
      </Field>
      <Field label={t("upstreams.fieldAuthHeader")}>
        <Input
          value={draft.auth_header}
          onChange={(e) => set("auth_header", e.target.value)}
          placeholder={t("upstreams.placeholderAuthHeader")}
        />
      </Field>
      <Field label={t("upstreams.fieldProxy")}>
        <Input
          value={draft.proxy ?? ""}
          onChange={(e) => set("proxy", e.target.value || null)}
          placeholder={t("upstreams.placeholderProxy")}
        />
      </Field>
      <Field label={t("upstreams.fieldProxyType")}>
        <Select
          value={draft.proxy_type}
          onValueChange={(value) =>
            set("proxy_type", value as UpstreamProxyType)
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="http">http</SelectItem>
            <SelectItem value="socks">socks</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      <Field label={t("upstreams.fieldMaxConcurrent")}>
        <NumberInput
          value={draft.max_concurrent_requests ?? null}
          onChange={(n) => set("max_concurrent_requests", n)}
          placeholder={t("common.unlimited")}
        />
      </Field>
      <Field label={t("upstreams.fieldRpm")}>
        <NumberInput
          value={draft.rpm ?? null}
          onChange={(n) => set("rpm", n)}
          placeholder={t("common.unlimited")}
        />
      </Field>
      <Field label={t("upstreams.fieldTpm")}>
        <NumberInput
          value={draft.tpm ?? null}
          onChange={(n) => set("tpm", n)}
          placeholder={t("common.unlimited")}
        />
      </Field>
    </div>
  );
}

function Field({
  label,
  full,
  children,
}: {
  label: string;
  full?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className={full ? "md:col-span-2" : undefined}>
      <Label>{label}</Label>
      <div className="mt-1.5">{children}</div>
    </div>
  );
}

function sanitize(draft: UpstreamConfig): UpstreamConfig {
  return {
    ...draft,
    name: draft.name?.trim() || null,
    url: draft.url.trim(),
    api_key: draft.api_key.trim(),
    auth_header: draft.auth_header.trim() || "Authorization",
    proxy: draft.proxy?.trim() || null,
  };
}

function upstreamRowKey(upstream: UpstreamConfig) {
  return [
    upstream.name ?? "",
    upstreamKey(upstream),
    upstream.proxy ?? "",
    upstream.proxy_type,
    upstream.max_concurrent_requests ?? "",
    upstream.rpm ?? "",
    upstream.tpm ?? "",
  ].join("\u0001");
}
