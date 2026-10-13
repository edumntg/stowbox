import { For, Show } from "solid-js";
import type { AgentStatus, Machine, MachineStatus, PowerAction } from "../bridge/types";
import { actions, state } from "../state/store";
import * as I from "../icons";

export function squareState(s: MachineStatus): string {
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

export function agentSquareState(s: AgentStatus): string {
  switch (s.kind) {
    case "starting":
    case "thinking":
    case "tool":
      return "busy";
    case "done":
      return "done";
    case "failed":
      return "failed";
    case "stopped":
      return "stopped";
    default:
      return "idle";
  }
}

export const StatusSquare = (p: { status: MachineStatus }) => <span class="sq" data-state={squareState(p.status)} />;
export const AgentSquare = (p: { status: AgentStatus }) => <span class="sq" data-state={agentSquareState(p.status)} />;

/** The app mark: a box seen from above, lid in the accent colour. */
export const BrandMark = () => (
  <svg class="brand-mark" viewBox="0 0 24 24" aria-hidden="true">
    <path d="M3 8l9-4 9 4-9 4z" fill="currentColor" />
    <path d="M3 8l9 4v9l-9-4z" fill="currentColor" opacity="0.55" />
    <path d="M21 8l-9 4v9l9-4z" fill="currentColor" opacity="0.35" />
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
        { action: "hibernate", label: "Hibernate", icon: I.Hibernate, title: "Write memory to disk, pay for disk only" },
        { action: "stop", label: "Stop", icon: I.Stop, title: "Stop the machine; processes are lost, disk stays" },
        { action: "reboot", label: "Reboot", icon: I.Refresh, title: "Cold restart" },
      ];
    case "suspended":
      return [
        { action: "resume", label: "Resume", icon: I.Play, title: "Resume from standby" },
        { action: "hibernate", label: "Hibernate", icon: I.Hibernate, title: "Write memory to disk, pay for disk only" },
        { action: "stop", label: "Stop", icon: I.Stop, title: "Stop the machine" },
      ];
    case "hibernated":
      return [
        { action: "wake", label: "Wake", icon: I.Wake, title: "Wake with processes intact" },
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

export const PowerActions = (p: { machine: Machine; quiet?: boolean }) => {
  const busy = () => !!state.pending[p.machine.id];
  return (
    <For each={powerButtons(p.machine)}>
      {(b) => (
        <button class="btn" classList={{ quiet: !!p.quiet }} title={b.title} disabled={busy()} onClick={() => void actions.power(p.machine.id, b.action)}>
          <b.icon />
          {b.label}
        </button>
      )}
    </For>
  );
};

export const Busy = (p: { when: boolean; label?: string }) => (
  <Show when={p.when}>
    <span class="faint">{p.label ?? "Working…"}</span>
  </Show>
);
