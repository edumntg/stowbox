import { For } from "solid-js";
import type { AgentMeta, Machine, MachineStatus, PowerAction } from "../bridge/types";
import { actions, state } from "../state/store";
import * as I from "../icons";
import { agentPhase, agentPhaseLabel, type AgentPhase } from "./format";

export function dotState(s: MachineStatus): string {
  switch (s) {
    case "running":
      return "running";
    case "suspended":
      return "standby";
    case "hibernated":
      return "hibernated";
    case "failed":
      return "failed";
    case "pending":
    case "starting":
    case "migrating":
      return "pending";
    default:
      return "stopped";
  }
}

export const phaseDot: Record<AgentPhase, string> = { working: "busy", waiting: "done", running: "running", stopped: "stopped", unknown: "idle" };

export const Dot = (p: { status: MachineStatus }) => <span class="dot" data-state={dotState(p.status)} />;
/** The agent as its box last showed it. */
export const AgentDot = (p: { agent: AgentMeta }) => {
  const phase = () => agentPhase(state.live[p.agent.id]);
  return <span class="dot" data-state={phaseDot[phase()]} title={agentPhaseLabel[phase()]} />;
};

/** The app mark: one cube, one stroke. */
export const BrandMark = (p: { size?: number }) => (
  <svg class="brand-mark" viewBox="0 0 24 24" width={p.size ?? 15} height={p.size ?? 15} fill="none" stroke="currentColor" stroke-width="1.75" stroke-linejoin="round" stroke-linecap="round" aria-hidden="true">
    <path d="M12 3l7.5 4.2v8.6L12 21l-7.5-5.2V7.2z" />
    <path d="M4.5 7.2 12 11.4l7.5-4.2" />
    <path d="M12 11.4V21" />
  </svg>
);

interface PowerButton {
  action: PowerAction;
  label: string;
  icon: (p: I.IconProps) => any;
  title: string;
}

export function powerButtons(m: Machine): PowerButton[] {
  switch (m.status) {
    case "running":
      return [
        { action: "pause", label: "Standby", icon: I.Pause, title: "Keep memory warm, stop paying for vCPU" },
        { action: "hibernate", label: "Hibernate", icon: I.Moon, title: "Write memory to disk, pay for disk only" },
        { action: "stop", label: "Stop", icon: I.Stop, title: "Stop the machine; processes are lost, disk stays" },
        { action: "reboot", label: "Reboot", icon: I.Refresh, title: "Cold restart" },
      ];
    case "suspended":
      return [
        { action: "resume", label: "Resume", icon: I.Play, title: "Resume from standby" },
        { action: "hibernate", label: "Hibernate", icon: I.Moon, title: "Write memory to disk, pay for disk only" },
        { action: "stop", label: "Stop", icon: I.Stop, title: "Stop the machine" },
      ];
    case "hibernated":
      return [
        { action: "wake", label: "Wake", icon: I.Sun, title: "Wake with processes intact" },
        { action: "stop", label: "Stop", icon: I.Stop, title: "Stop the machine" },
      ];
    case "stopped":
      return [{ action: "start", label: "Start", icon: I.Play, title: "Start the machine" }];
    case "failed":
      return [{ action: "reboot", label: "Reboot", icon: I.Refresh, title: "Try a cold restart" }];
    default:
      return [];
  }
}

export const PowerActions = (p: { machine: Machine; ghost?: boolean; size?: "sm" }) => {
  const busy = () => !!state.pending[p.machine.id];
  return (
    <For each={powerButtons(p.machine)}>
      {(b) => (
        <button class="btn" classList={{ ghost: !!p.ghost, sm: p.size === "sm" }} title={b.title} disabled={busy()} onClick={() => void actions.power(p.machine.id, b.action)}>
          <b.icon />
          {b.label}
        </button>
      )}
    </For>
  );
};

export const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
/** WebView2 reports "Win32" in navigator.platform and "Windows" in the user agent. */
export const isWindows = typeof navigator !== "undefined" && /Win/i.test(navigator.platform || navigator.userAgent);
