// i18next initialisation. English is the source of truth (`en.ts` exports
// `Translations` and `zh.ts` is typed against it so missing keys break the
// TypeScript build instead of silently falling back at runtime).

import i18n from "i18next";
import LanguageDetector from "i18next-browser-languagedetector";
import { initReactI18next } from "react-i18next";

import { api } from "@/lib/api";

import en from "./locales/en";
import zh from "./locales/zh";

export type SupportedLanguage = "en" | "zh";

export const SUPPORTED_LANGUAGES: { value: SupportedLanguage; label: string }[] = [
  { value: "en", label: "EN" },
  { value: "zh", label: "中" },
];

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: "en",
    supportedLngs: ["en", "zh"],
    interpolation: { escapeValue: false },
    detection: {
      // Order matters: persisted choice wins over browser hint. The
      // localStorage layer keeps the choice across webview reloads; the
      // Rust `desktop_preferences.toml` file is the durable backup synced
      // by `bootstrapI18n` below.
      order: ["localStorage", "navigator"],
      lookupLocalStorage: "open-promux.lang",
      caches: ["localStorage"],
    },
    resources: {
      en: { translation: en },
      zh: { translation: zh },
    },
  });

/**
 * Hydrate from the Rust-side preferences file. Called from `main.tsx`
 * before the React tree mounts so the first paint already shows the
 * persisted language. Safe to call outside Tauri (the invoke rejects and
 * we keep the localStorage / navigator detected language).
 */
export async function bootstrapI18n(): Promise<void> {
  try {
    const prefs = await api.getPreferences();
    const lang = normaliseLanguage(prefs.language ?? undefined);
    if (lang && i18n.resolvedLanguage !== lang) {
      await i18n.changeLanguage(lang);
    }
  } catch {
    // Either not running inside Tauri or first run – keep detector result.
  }
}

export function setLanguage(lang: SupportedLanguage): void {
  void i18n.changeLanguage(lang).then(() => {
    // Best-effort persistence; ignore failures so the in-memory switch
    // still works on environments without the Rust backend.
    void api.savePreferences({ language: lang }).catch(() => undefined);
  });
}

export function currentLanguage(): SupportedLanguage {
  const raw = i18n.resolvedLanguage ?? i18n.language ?? "en";
  return raw.startsWith("zh") ? "zh" : "en";
}

function normaliseLanguage(value: string | undefined): SupportedLanguage | null {
  if (!value) return null;
  return value.startsWith("zh") ? "zh" : value.startsWith("en") ? "en" : null;
}
