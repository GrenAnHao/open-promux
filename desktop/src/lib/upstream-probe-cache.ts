// Module-level cache for per-upstream model lists shown on the
// Dashboard.
//
// Background
// ----------
// Radix `Tabs.Content` unmounts inactive panels by default, so every time
// the user returns to the Dashboard tab the `UpstreamRow` components
// re-mount and their `useEffect` would re-issue `fetch_upstream_models`
// against every upstream. That's both unnecessary and surprising –
// switching tabs should feel instantaneous and not perform network I/O
// behind the user's back.
//
// This cache survives component unmounts and is keyed by the set of
// upstream fields that actually affect `/v1/models` responses: URL,
// credential, auth header name and API format. `name` is intentionally
// not part of the key because renaming an upstream doesn't change the
// remote it points at.
//
// Entries are mutated only from:
//   * The initial "load once" fetch triggered when a row mounts with no
//     cached entry for its key.
//   * The user clicking the row-local refresh button (force=true).
//   * The user picking a different model in the row dropdown (just
//     updates `selected` in place, keeps the model list).
//
// Chat-probe outcomes are surfaced as toasts and intentionally not
// cached: they represent a one-shot action the user just took, not
// reusable shared state.
//
// Notifications use a tiny listener set – good enough for the handful of
// rows rendered on the Dashboard and keeps us free of a store dependency
// (zustand / jotai / etc.).

import type { UpstreamConfig } from "./types";

export interface ModelsEntry {
  /** Models returned by the last successful fetch, or `[]` on failure. */
  models: string[];
  /** Error message from the last attempt, or `null` on success. */
  error: string | null;
  /** The id currently selected in the dropdown. Empty string = none. */
  selected: string;
  /** Wall-clock ms when the entry was last written. Unused for TTL but
   *  handy for future staleness indicators. */
  fetchedAt: number;
}

type Listener = () => void;

const modelsCache = new Map<string, ModelsEntry>();
const listeners = new Set<Listener>();

/**
 * Derive a stable cache key for an upstream.
 *
 * Uses `\u0001` as a separator because the real values may legitimately
 * contain any ASCII punctuation; a non-printable byte is guaranteed never
 * to appear inside a URL, header name or API key.
 */
export function upstreamKey(u: UpstreamConfig): string {
  return [u.api_format, u.auth_header, u.url, u.api_key].join("\u0001");
}

export function getModelsEntry(key: string): ModelsEntry | undefined {
  return modelsCache.get(key);
}

export function setModelsEntry(key: string, entry: ModelsEntry): void {
  modelsCache.set(key, entry);
  emit();
}

export function patchSelected(key: string, selected: string): void {
  const prev = modelsCache.get(key);
  if (!prev) {
    // No models fetched yet – remember the selection so it survives a
    // remount even before the first fetch completes.
    modelsCache.set(key, {
      models: [],
      error: null,
      selected,
      fetchedAt: 0,
    });
  } else if (prev.selected !== selected) {
    modelsCache.set(key, { ...prev, selected });
  } else {
    return;
  }
  emit();
}

export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function emit(): void {
  listeners.forEach((l) => {
    try {
      l();
    } catch {
      // Listeners are simple state setters; a throw here would only mean
      // React has been torn down mid-notify – safe to swallow.
    }
  });
}
