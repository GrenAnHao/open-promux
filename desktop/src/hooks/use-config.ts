import { useCallback, useEffect, useState } from "react";

import { api } from "@/lib/api";
import { Config, emptyConfig, normalizeConfig } from "@/lib/types";

interface UseConfigState {
  config: Config;
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
  save: (next: Config) => Promise<void>;
}

export function useConfig(): UseConfigState {
  const [config, setConfig] = useState<Config>(emptyConfig());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await api.loadConfig();
      setConfig(normalizeConfig(next));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const save = useCallback(async (next: Config) => {
    await api.saveConfig(next);
    setConfig(normalizeConfig(next));
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return { config, loading, error, reload, save };
}
