import { useVirtualizer } from "@tanstack/react-virtual";
import { Copy, Eraser } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Panel } from "@/components/ui/panel";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useLogs } from "@/hooks/use-logs";
import type { LogLine } from "@/lib/types";
import { cn, formatTimestamp } from "@/lib/utils";

const LEVELS: LogLine["level"][] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];

const levelColor: Record<LogLine["level"], string> = {
  TRACE: "text-ink-700",
  DEBUG: "text-ink-500",
  INFO: "text-mint-300",
  WARN: "text-amber-400",
  ERROR: "text-coral-400",
};

// Single-line height in pixels for the virtual list. Matches the row's
// font-mono 12px text * 1.55 line-height + 2px vertical padding.
const ROW_HEIGHT = 22;

export function LogsPage() {
  const { t } = useTranslation();
  const { lines, clear } = useLogs();
  const [minLevel, setMinLevel] = useState<LogLine["level"]>("INFO");
  const [filter, setFilter] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  const minIndex = LEVELS.indexOf(minLevel);
  const filtered = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    return lines.filter((line) => {
      if (LEVELS.indexOf(line.level) < minIndex) return false;
      if (!needle) return true;
      return (
        line.message.toLowerCase().includes(needle) ||
        line.target.toLowerCase().includes(needle)
      );
    });
  }, [lines, minIndex, filter]);

  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  useEffect(() => {
    if (!autoScroll || filtered.length === 0) return;
    // Scroll the virtualizer to the last item; reads the freshly-mounted
    // list size and avoids a manual `scrollHeight` reflow.
    virtualizer.scrollToIndex(filtered.length - 1, { align: "end" });
  }, [filtered.length, autoScroll, virtualizer]);

  const copyAll = async () => {
    const text = filtered
      .map(
        (line) =>
          `${formatTimestamp(line.ts_millis)} ${line.level.padEnd(5)} ${line.target} ${line.message}`,
      )
      .join("\n");
    try {
      await navigator.clipboard.writeText(text);
      toast.success(t("logs.copied", { count: filtered.length }));
    } catch {
      toast.error(t("logs.clipboardUnavailable"));
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col p-4">
      <Panel
        className="flex flex-1 min-h-0 flex-col"
        bodyClassName="flex flex-1 min-h-0 flex-col p-0"
        title={t("logs.title")}
        trailing={
          <>
            <Select
              value={minLevel}
              onValueChange={(value) => setMinLevel(value as LogLine["level"])}
            >
              <SelectTrigger className="h-7 w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {LEVELS.map((level) => (
                  <SelectItem key={level} value={level}>
                    {level}+
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              className="h-7 w-48"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("logs.placeholderFilter")}
            />
            <Button
              size="sm"
              variant={autoScroll ? "primary" : "ghost"}
              onClick={() => setAutoScroll((s) => !s)}
            >
              {t("logs.tail")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void copyAll()}>
              <Copy className="size-3.5" />
              {t("logs.copy")}
            </Button>
            <Button size="sm" variant="danger" onClick={() => void clear()}>
              <Eraser className="size-3.5" />
              {t("logs.clear")}
            </Button>
          </>
        }
      >
        <div
          ref={scrollRef}
          className="scrollbar-thin flex-1 overflow-y-auto bg-carbon-900/50 px-4"
          onScroll={(e) => {
            const { scrollTop, scrollHeight, clientHeight } =
              e.currentTarget;
            const atBottom = scrollHeight - (scrollTop + clientHeight) < 8;
            if (!atBottom && autoScroll) setAutoScroll(false);
          }}
        >
          {filtered.length === 0 ? (
            <p className="p-4 font-mono text-sm text-ink-500">
              {t("logs.empty")}
            </p>
          ) : (
            <div
              // Spacer that gives the scroll viewport its true height; the
              // virtualizer mounts a small slice of <li>s positioned
              // absolutely inside this spacer.
              style={{ height: virtualizer.getTotalSize(), position: "relative" }}
              className="py-2 font-mono text-[12px] leading-[1.55]"
            >
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const line = filtered[virtualRow.index];
                if (!line) return null;
                return (
                  <div
                    key={virtualRow.key}
                    data-index={virtualRow.index}
                    style={{
                      position: "absolute",
                      top: 0,
                      left: 0,
                      width: "100%",
                      height: virtualRow.size,
                      transform: `translateY(${virtualRow.start}px)`,
                    }}
                    className="grid grid-cols-[80px_60px_1fr] gap-2"
                    title={`${line.target} ${line.message}`}
                  >
                    <span className="truncate text-ink-700">
                      {formatTimestamp(line.ts_millis)}
                    </span>
                    <span className={cn("truncate uppercase", levelColor[line.level])}>
                      {line.level}
                    </span>
                    <span className="truncate text-ink-200">
                      <span className="text-ink-500">{line.target}</span>{" "}
                      {line.message}
                    </span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </Panel>
    </div>
  );
}
