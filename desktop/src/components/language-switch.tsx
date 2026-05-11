import { useTranslation } from "react-i18next";

import {
  SUPPORTED_LANGUAGES,
  type SupportedLanguage,
  currentLanguage,
  setLanguage,
} from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * Two-button toggle for switching between English and Chinese.
 * Choice is persisted via `i18next-browser-languagedetector` (localStorage).
 */
export function LanguageSwitch({ className }: { className?: string }) {
  const { i18n } = useTranslation();
  // `i18n.language` updates trigger a re-render via the hook; we still call
  // `currentLanguage()` so the active button reflects the resolved value
  // (e.g. when navigator reports `zh-CN` we want to highlight `中`).
  void i18n.language;
  const active = currentLanguage();
  return (
    <div className={cn("inline-flex items-stretch border border-carbon-500", className)}>
      {SUPPORTED_LANGUAGES.map((entry) => (
        <LangButton
          key={entry.value}
          value={entry.value}
          label={entry.label}
          active={active === entry.value}
        />
      ))}
    </div>
  );
}

interface LangButtonProps {
  value: SupportedLanguage;
  label: string;
  active: boolean;
}

function LangButton({ value, label, active }: LangButtonProps) {
  return (
    <button
      type="button"
      onClick={() => setLanguage(value)}
      className={cn(
        "px-2 py-1 font-mono text-[11px] uppercase tracking-[0.18em] transition-colors",
        active
          ? "bg-mint-400/15 text-mint-300"
          : "text-ink-500 hover:text-ink-200",
      )}
    >
      {label}
    </button>
  );
}
