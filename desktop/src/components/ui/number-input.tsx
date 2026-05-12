import { ChevronDown, ChevronUp } from "lucide-react";

import { cn } from "@/lib/utils";

export interface NumberInputProps {
  value: number | null;
  onChange: (next: number | null) => void;
  placeholder?: string;
  /** Inclusive lower bound. `null`-typed values are still allowed. */
  min?: number;
  /** Inclusive upper bound. */
  max?: number;
  /** Increment / decrement amount used by the up / down buttons. */
  step?: number;
  className?: string;
  disabled?: boolean;
}

/**
 * Number input that hides the platform default spinner (which renders
 * inconsistently in WebView2 dark mode) and replaces it with a square,
 * mint-on-carbon stepper to match the rest of the terminal UI.
 *
 * The native `<input>` is still used so keyboard input, paste, locale
 * formatting and `tabindex` behaviour stay identical to a plain text
 * field; the buttons are `tabindex=-1` so keyboard focus walks past them.
 */
export function NumberInput({
  value,
  onChange,
  placeholder,
  min = 0,
  max,
  step = 1,
  className,
  disabled,
}: NumberInputProps) {
  const adjust = (delta: number) => {
    if (disabled) return;
    const current = value ?? 0;
    let next = current + delta;
    if (typeof min === "number" && next < min) next = min;
    if (typeof max === "number" && next > max) next = max;
    onChange(next);
  };

  return (
    <div className={cn("relative", className)}>
      <input
        type="number"
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        value={value === null ? "" : String(value)}
        placeholder={placeholder}
        onChange={(e) => {
          const raw = e.target.value.trim();
          if (raw === "") {
            onChange(null);
            return;
          }
          const parsed = Number(raw);
          if (!Number.isFinite(parsed)) return;
          if (typeof min === "number" && parsed < min) return;
          if (typeof max === "number" && parsed > max) return;
          onChange(parsed);
        }}
        className={cn(
          "h-8 w-full bg-carbon-800/60 border border-carbon-500 pl-2.5 pr-8 font-mono text-[13px] text-ink-100 placeholder:text-ink-700",
          "transition-colors hover:border-carbon-400 focus:border-mint-400/70 focus:outline-none focus:bg-carbon-800",
          "disabled:opacity-40",
        )}
      />
      <div
        // The stepper sits flush against the input's right border. Two
        // square buttons stacked vertically, separated by a 1px divider.
        className="pointer-events-none absolute inset-y-0 right-0 flex w-6 flex-col border-l border-carbon-500"
        aria-hidden
      >
        <button
          type="button"
          tabIndex={-1}
          aria-label="Increment"
          disabled={disabled}
          onClick={() => adjust(step)}
          className="pointer-events-auto flex flex-1 items-center justify-center text-ink-500 transition-colors hover:bg-carbon-700/80 hover:text-mint-300 active:bg-carbon-600 disabled:opacity-40"
        >
          <ChevronUp className="size-3" strokeWidth={2.5} />
        </button>
        <button
          type="button"
          tabIndex={-1}
          aria-label="Decrement"
          disabled={disabled}
          onClick={() => adjust(-step)}
          className="pointer-events-auto flex flex-1 items-center justify-center border-t border-carbon-500 text-ink-500 transition-colors hover:bg-carbon-700/80 hover:text-mint-300 active:bg-carbon-600 disabled:opacity-40"
        >
          <ChevronDown className="size-3" strokeWidth={2.5} />
        </button>
      </div>
    </div>
  );
}
