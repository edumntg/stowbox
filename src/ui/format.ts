import type { AgentStatus, Machine, MachineStatus } from "../bridge/types";

const GIB = 1024 ** 3;

export function money(micro: number, currency: string): string {
  const symbol = currency === "eur" ? "€" : currency === "usd" ? "$" : currency.toUpperCase() + " ";
  const units = micro / 1_000_000;
  const abs = Math.abs(units);
  let body: string;
  if (abs >= 100) body = abs.toFixed(0);
  else if (abs >= 1) body = abs.toFixed(2);
  else if (abs >= 0.01) body = abs.toFixed(3);
  else if (abs === 0) body = "0";
  else body = abs.toFixed(4);
  return (units < 0 ? "-" : "") + symbol + body;
}

export function usd(amount: number): string {
  if (amount === 0) return "$0";
  if (amount < 0.01) return "$" + amount.toFixed(4);
  if (amount < 1) return "$" + amount.toFixed(3);
  return "$" + amount.toFixed(2);
}

export function gib(bytes: number): string {
  const g = bytes / GIB;
  return (Number.isInteger(g) ? g : g.toFixed(1)) + " GiB";
}

export function sizeLabel(m: Pick<Machine, "vcpu" | "memoryBytes">): string {
  return `${m.vcpu} vCPU · ${gib(m.memoryBytes)}`;
}

export function compactSize(m: Pick<Machine, "vcpu" | "memoryBytes">): string {
  return `${m.vcpu}c ${Math.round(m.memoryBytes / GIB)}G`;
}

export const statusLabel: Record<MachineStatus, string> = {
  pending: "Pending",
  starting: "Starting",
  running: "Running",
  suspended: "Standby",
  hibernated: "Hibernated",
  stopped: "Stopped",
  failed: "Failed",
  migrating: "Migrating",
  unknown: "Unknown",
};

export function agentStatusLabel(s: AgentStatus): string {
  switch (s.kind) {
    case "idle":
      return "Not started";
    case "starting":
      return "Starting";
    case "thinking":
      return "Thinking";
    case "tool":
      return "Running " + s.name;
    case "reconnecting":
      return "Reconnecting";
    case "done":
      return "Finished";
    case "failed":
      return "Failed";
    case "stopped":
      return "Stopped";
  }
}

export function isBusy(s: AgentStatus): boolean {
  return s.kind === "starting" || s.kind === "thinking" || s.kind === "tool" || s.kind === "reconnecting";
}

export function relTime(ms: number, now = Date.now()): string {
  if (!ms) return "never";
  const d = Math.max(0, now - ms);
  const s = Math.round(d / 1000);
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} h ago`;
  const days = Math.round(h / 24);
  return `${days} d ago`;
}

export function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  if (m < 60) return `${m} min ${rs} s`;
  const h = Math.floor(m / 60);
  return `${h} h ${m - h * 60} min`;
}

export function hours(seconds: number): string {
  const h = seconds / 3600;
  if (h < 1) return `${Math.round(seconds / 60)} min`;
  return `${h.toFixed(1)} h`;
}

export function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return (n / 1000).toFixed(1).replace(/\.0$/, "") + "k";
  return (n / 1_000_000).toFixed(2) + "M";
}

export function pct(n: number): string {
  return `${Math.round(n)}%`;
}

/** Uptime the way `uptime` prints it, trimmed to two units. */
export function uptime(secs: number): string {
  if (secs < 60) return "<1m";
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}
