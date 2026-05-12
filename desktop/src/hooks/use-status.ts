import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { ServerStatus } from "@/lib/types";

const POLL_INTERVAL_MS = 1500;

/**
 * Polls the embedded server status. Polling (instead of pushed events) keeps
 * the contract trivial: the renderer never has to handle reconnection logic
 * and stale data is bounded to the polling interval.
 */
export function useStatus() {
  const [status, setStatus] = useState<ServerStatus>({
    running: false,
    uptime_seconds: 0,
  });
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
      let next: ServerStatus = { running: false, uptime_seconds: 0 };
      try {
        next = await api.getStatus();
      } catch {
        next = { running: false, uptime_seconds: 0 };
      }
      if (!cancelled) {
        setStatus(next);
        timer = setTimeout(poll, POLL_INTERVAL_MS);
      }
    };

    poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [refreshKey]);

  return {
    status,
    refresh: () => setRefreshKey((k) => k + 1),
  };
}
