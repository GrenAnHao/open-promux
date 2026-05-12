import { useCallback, useEffect, useSyncExternalStore } from "react";

import { api } from "@/lib/api";
import { Config, emptyConfig, normalizeConfig } from "@/lib/types";

interface UseConfigState {
  config: Config;
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
  save: (next: Config) => Promise<void>;
}

interface ConfigSnapshot extends UseConfigState {
  initialized: boolean;
}

type Listener = () => void;

let snapshot: ConfigSnapshot = {
  config: emptyConfig(),
  loading: true,
  error: null,
  initialized: false,
  reload: async () => reloadConfig(),
  save: async (next: Config) => saveConfig(next),
};

let reloadPromise: Promise<void> | null = null;
const listeners = new Set<Listener>();

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): ConfigSnapshot {
  return snapshot;
}

function setSnapshot(next: Partial<ConfigSnapshot>): void {
  snapshot = { ...snapshot, ...next };
  listeners.forEach((listener) => listener());
}

async function reloadConfig(): Promise<void> {
  if (reloadPromise) return reloadPromise;
  setSnapshot({ loading: true, error: null });
  reloadPromise = (async () => {
    try {
      const next = await api.loadConfig();
      setSnapshot({
        config: normalizeConfig(next),
        error: null,
        initialized: true,
      });
    } catch (err) {
      setSnapshot({ error: String(err), initialized: true });
    } finally {
      setSnapshot({ loading: false });
      reloadPromise = null;
    }
  })();
  return reloadPromise;
}

async function saveConfig(next: Config): Promise<void> {
  await api.saveConfig(next);
  setSnapshot({
    config: normalizeConfig(next),
    loading: false,
    error: null,
    initialized: true,
  });
}

export function useConfig(): UseConfigState {
  const current = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const reload = useCallback(() => reloadConfig(), []);
  const save = useCallback((next: Config) => saveConfig(next), []);

  useEffect(() => {
    if (!snapshot.initialized) {
      void reloadConfig();
    }
  }, []);

  return {
    config: current.config,
    loading: current.loading,
    error: current.error,
    reload,
    save,
  };
}
