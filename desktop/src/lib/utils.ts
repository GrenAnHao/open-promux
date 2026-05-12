import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0s";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function formatTimestamp(ms: number): string {
  if (!ms) return "--:--:--";
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

/**
 * Wildcard listen hosts (`0.0.0.0`, `::`) cannot be opened in a browser.
 * For display purposes we resolve them to a loopback (`127.0.0.1` / `::1`)
 * and return a hint string that mentions the original "any-interface" host.
 *
 * Returned `display` is intended for the prominent `host:port` slot; `hint`
 * (if any) is meant for a `title=` tooltip so power users still know the
 * actual bind interface.
 */
export interface ResolvedAddress {
  display: string;
  hint?: string;
}

export function resolveDisplayAddress(
  address: string | null | undefined,
  port: number | null | undefined,
): ResolvedAddress {
  const portPart = port ?? "?";
  if (!address) {
    return { display: `127.0.0.1:${portPart}` };
  }
  if (address === "0.0.0.0") {
    return {
      display: `127.0.0.1:${portPart}`,
      hint: `listening on 0.0.0.0:${portPart} (any IPv4 interface)`,
    };
  }
  if (address === "::") {
    return {
      display: `[::1]:${portPart}`,
      hint: `listening on [::]:${portPart} (any IPv6 interface)`,
    };
  }
  // IPv6 literal that isn't already bracketed.
  if (address.includes(":") && !address.startsWith("[")) {
    return { display: `[${address}]:${portPart}` };
  }
  return { display: `${address}:${portPart}` };
}
