import type { AgentLive, Machine, MachineStatus, SpendCheck } from "../bridge/types";

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

export const currencySymbol = (currency: string) => (currency === "eur" ? "€" : currency === "usd" ? "$" : currency.toUpperCase());

/** Where the day's spend figure comes from, in one sentence. */
export function spendLine(c: SpendCheck): string {
  if (c.basis === "metered") {
    return `boxd has metered ${money(c.totalMicro, c.currency)} since midnight` + (c.complete ? "." : "; metering lags up to 25 minutes, so it can still grow.");
  }
  return `Estimate: at the current ${money(c.hourlyMicro, c.currency)}/h, a full day costs ${money(c.amountMicro, c.currency)}. boxd's metered usage is not readable with this key (${c.why}).`;
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

/** Amounts in several currencies, summed per currency and never across
 * them: "€0.18/h + $0.04/h". `suffix` goes after each part. */
export function moneyByCurrency(items: { micro: number; currency: string }[], fallbackCurrency: string, suffix = ""): string {
  const by = new Map<string, number>();
  for (const i of items) by.set(i.currency.toLowerCase(), (by.get(i.currency.toLowerCase()) ?? 0) + i.micro);
  if (by.size === 0) return money(0, fallbackCurrency) + suffix;
  return [...by].map(([c, v]) => money(v, c) + suffix).join(" + ");
}

export const statusLabel: Record<MachineStatus, string> = {
  pending: "Pending",
  starting: "Starting",
  running: "Running",
  suspended: "Standby",
  hibernated: "Hibernated",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Failed",
  migrating: "Migrating",
  unknown: "Unknown",
};

/** Where an agent is, from its box's last watch round. "running" is a
 * harness without hooks (Codex), or Claude Code before its first hook;
 * "unknown" means nothing has watched the box since the app started. */
export type AgentPhase = "working" | "waiting" | "running" | "stopped" | "unknown";

export function agentPhase(l: AgentLive | undefined): AgentPhase {
  if (!l) return "unknown";
  if (!l.running) return "stopped";
  return l.state ?? "running";
}

export const agentPhaseLabel: Record<AgentPhase, string> = {
  working: "Working",
  waiting: "Waiting for you",
  running: "Running",
  stopped: "Not running",
  unknown: "Not checked yet",
};

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

export function hours(seconds: number): string {
  const h = seconds / 3600;
  if (h < 1) return `${Math.round(seconds / 60)} min`;
  return `${h.toFixed(1)} h`;
}

export function pct(n: number): string {
  return `${Math.round(n)}%`;
}

/** A span of time the way `uptime` prints it, trimmed to two units. */
export function span(secs: number): string {
  if (secs < 60) return "<1m";
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

/** "512 B", "48 KB", "1.2 MB" for attached files. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
