import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import type { CreateSpec, Harness, Machine } from "../bridge/types";
import { actions, agentsForMachine, capsOf, creatableProviders, machineById, multiProvider, providerLabel, state } from "../state/store";
import * as I from "../icons";
import { boxHourly, money, sizeLabel, statusLabel } from "./format";

const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
/** A new box from the composer stops on its own after this, where the provider has auto-stop. */
const AUTO_STOP_SECS = 4 * 3600;

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

/** One place a task can run: a box that exists, or a new one on a provider. */
interface Place {
  value: string;
  label: string;
  /** The line under the composer: where it runs and what it costs. */
  hint: string;
  box: { existing: string } | { create: CreateSpec };
}

const usable = (m: Machine) => m.status === "running" || m.status === "suspended" || m.status === "hibernated" || m.status === "starting";

/** The box to suggest: the running one with the latest agent activity, then any running, then one in standby. */
function suggested(): Machine | undefined {
  const last = (m: Machine) => Math.max(0, ...agentsForMachine(m.id).map((a) => a.updatedAt));
  const ms = state.machines.filter(usable);
  const rank = (m: Machine) => (m.status === "running" ? 0 : m.status === "starting" ? 1 : 2);
  return [...ms].sort((a, b) => rank(a) - rank(b) || last(b) - last(a))[0];
}

function newBoxPlace(providerId: string): Place | null {
  const caps = capsOf(providerId);
  if (!caps) return null;
  const on = multiProvider() ? ` on ${caps.label}` : "";
  if (!caps.customSize && caps.sizes.length > 0) {
    // The cheapest size on the menu: the composer is for getting going.
    const size = [...caps.sizes].sort((a, b) => a.hourlyMicro - b.hourlyMicro)[0];
    const price = money(size.hourlyMicro, caps.currency);
    const stops = caps.autoStop ? `, stops on its own after ${AUTO_STOP_SECS / 3600} h` : "";
    return {
      value: `new:${providerId}`,
      label: `New box${on} · ${size.id} · ${price}/h`,
      hint: `Creates a ${size.id} box on ${caps.label} (${sizeLabel(size).replace(" · ", ", ")}, ${price}/h while it runs${stops}), then opens the agent in it.`,
      box: { create: { provider: providerId, size: size.id, autoStopSecs: caps.autoStop ? AUTO_STOP_SECS : null, isolated: false } },
    };
  }
  const b = state.accounts[providerId]?.billing;
  const size = b && b.defaultVcpu ? ` · ${sizeLabel({ vcpu: b.defaultVcpu, memoryBytes: b.defaultMemoryBytes })}` : "";
  return {
    value: `new:${providerId}`,
    label: `New box${on}${size}`,
    hint: `Creates a box on ${caps.label} at the account's default size, then opens the agent in it.`,
    box: { create: { provider: providerId, isolated: false } },
  };
}

function boxPlace(m: Machine): Place {
  const where = multiProvider() ? ` · ${providerLabel(m.provider)}` : "";
  const state_ = m.status === "running" ? "" : ` · ${statusLabel[m.status].toLowerCase()}`;
  const wakes = m.status === "suspended" || m.status === "hibernated" ? " It wakes first, and bills as running from then." : "";
  return {
    value: m.id,
    label: `${m.name}${state_}${where}`,
    hint: `Runs in ${m.name}, ${statusLabel[m.status].toLowerCase()}, ${m.currency ? `${boxHourly(m)} now` : "no price"}.${wakes}`,
    box: { existing: m.id },
  };
}

/** The Fleet view's first thing: say what to do, and Stowbox finds or makes
 * the box and opens the agent with the prompt. Everything else the New agent
 * dialog offers is one click away under More options. */
export default function Composer() {
  const [prompt, setPrompt] = createSignal("");
  const [harness, setHarness] = createSignal<Harness>("claude-code");
  const [place, setPlace] = createSignal("");
  const [cwd, setCwd] = createSignal("");
  const [cwdEdited, setCwdEdited] = createSignal(false);
  const [step, setStep] = createSignal("");
  const [error, setError] = createSignal("");

  const places = createMemo(() => {
    const boxes = state.machines.filter(usable).map(boxPlace);
    const fresh = creatableProviders()
      .map((p) => newBoxPlace(p.id))
      .filter((p): p is Place => !!p);
    return { boxes, fresh, all: [...boxes, ...fresh] };
  });
  // The pick follows the fleet until the person chooses; a box that goes away falls back.
  createEffect(() => {
    const all = places().all;
    if (all.some((p) => p.value === place())) return;
    setPlace(suggested()?.id ?? all[0]?.value ?? "");
  });
  const chosen = () => places().all.find((p) => p.value === place());

  // The directory the box's latest agent worked in, until the person types one.
  createEffect(
    on(place, (v) => {
      if (cwdEdited()) return;
      const m = machineById(v);
      const recent = m ? [...agentsForMachine(m.id)].sort((a, b) => b.updatedAt - a.updatedAt).find((a) => a.cwd) : undefined;
      setCwd(recent?.cwd ?? "");
    }),
  );

  const busy = () => step() !== "";
  const start = async () => {
    const p = chosen();
    if (!prompt().trim() || !p || busy()) return;
    setError("");
    setStep("Starting…");
    try {
      await actions.startTask({ prompt: prompt(), harness: harness(), cwd: cwd().trim() || null, box: p.box }, setStep);
      setPrompt("");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStep("");
    }
  };
  const more = () =>
    actions.openDialog({ kind: "new-agent", machineId: "existing" in (chosen()?.box ?? {}) ? place() : undefined, prompt: prompt(), cwd: cwd(), harness: harness() });

  return (
    <form
      class="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void start();
      }}
    >
      <textarea
        id="task-prompt"
        class="composer-input"
        rows={3}
        placeholder="What should the agent do? For example: add a /health endpoint and a test for it"
        value={prompt()}
        disabled={busy()}
        onInput={(e) => setPrompt(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void start();
          }
        }}
      />
      <div class="composer-bar">
        <div class="seg" role="group" aria-label="Harness">
          <button type="button" aria-pressed={harness() === "claude-code"} onClick={() => setHarness("claude-code")}>
            Claude Code
          </button>
          <button type="button" aria-pressed={harness() === "codex"} onClick={() => setHarness("codex")}>
            Codex
          </button>
        </div>
        <select class="input composer-box" aria-label="Where it runs" value={place()} disabled={busy()} onChange={(e) => setPlace(e.currentTarget.value)}>
          <Show when={places().boxes.length > 0}>
            <optgroup label="Your boxes">
              <For each={places().boxes}>{(p) => <option value={p.value}>{p.label}</option>}</For>
            </optgroup>
          </Show>
          <optgroup label="A new box">
            <For each={places().fresh}>{(p) => <option value={p.value}>{p.label}</option>}</For>
          </optgroup>
        </select>
        <input
          class="input mono composer-dir"
          aria-label="Working directory"
          placeholder="directory (empty = home)"
          value={cwd()}
          disabled={busy()}
          onInput={(e) => {
            setCwd(e.currentTarget.value);
            setCwdEdited(true);
          }}
        />
        <span class="spacer" />
        <button type="button" class="btn ghost sm" disabled={busy()} onClick={more}>
          More options
        </button>
        <button type="submit" class="btn primary" disabled={!prompt().trim() || !chosen() || busy()} title={`${isMac ? "⌘" : "Ctrl"}+Enter`}>
          <I.Play /> {step() || "Start"}
        </button>
      </div>
      <Show when={error()} fallback={<p class="composer-hint">{chosen()?.hint ?? "Connect a provider in Settings to make a box."}</p>}>
        <div class="form-error">{error()}</div>
      </Show>
    </form>
  );
}
