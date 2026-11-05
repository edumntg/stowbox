import { For, Match, Show, Switch, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import type { Harness, Preset, VariableValues } from "../bridge/types";
import { presetVariables } from "../bridge/variables";
import { actions, machineById, state, type Dialog } from "../state/store";
import * as I from "../icons";
import { isBusy, money } from "./format";

const GIB = 1024 ** 3;

function Modal(p: { title: string; children: JSX.Element; onClose: () => void }) {
  let panel!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") p.onClose();
  };
  onMount(() => {
    document.addEventListener("keydown", onKey);
    const first = panel.querySelector<HTMLElement>("input, textarea, select, button");
    first?.focus();
  });
  onCleanup(() => document.removeEventListener("keydown", onKey));
  return (
    <div class="overlay" onMouseDown={(e) => e.target === e.currentTarget && p.onClose()}>
      <div class="dialog" role="dialog" aria-modal="true" aria-label={p.title} ref={panel}>
        <h2>{p.title}</h2>
        {p.children}
      </div>
    </div>
  );
}

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

/** The values a preset's variables start with: their defaults. */
const defaultValues = (p: Preset | undefined): VariableValues => Object.fromEntries((p ? presetVariables(p) : []).map((v) => [v.name, v.default]));

/** The first variable left without a value or a default, as an error. */
const missingValue = (p: Preset | undefined, values: VariableValues) => {
  const v = (p ? presetVariables(p) : []).find((x) => !values[x.name] && !x.default);
  return v ? `give ${v.name} a value` : "";
};

/** One input per `{{name}}` in the preset's commands. */
const VariableFields = (p: { preset: Preset; values: VariableValues; onInput: (name: string, value: string) => void }) => (
  <Show when={presetVariables(p.preset).length > 0}>
    <div class="var-rows">
      <For each={presetVariables(p.preset)}>
        {(v) => (
          <div class="var-row">
            <label class="mono" for={`var-${v.name}`}>
              {v.name}
            </label>
            <input
              id={`var-${v.name}`}
              class="input mono"
              spellcheck={false}
              autocomplete="off"
              placeholder={v.default ? `default: ${v.default}` : "required"}
              value={p.values[v.name] ?? ""}
              onInput={(e) => p.onInput(v.name, e.currentTarget.value)}
            />
          </div>
        )}
      </For>
    </div>
    <p class="hint">Each value goes into the commands as one quoted word. Values are kept with the run and show in its log; put tokens in secrets.</p>
  </Show>
);

const NewBox = () => {
  const billing = () => state.account?.billing;
  const rates = () => state.rateCard;
  const sizes = () => [
    { vcpu: 1, mem: 4 },
    { vcpu: 2, mem: 8 },
    { vcpu: 4, mem: 16 },
  ].filter((s) => !billing() || billing()!.ceilingVcpu === 0 || s.vcpu <= billing()!.ceilingVcpu);
  const [name, setName] = createSignal("");
  const [vcpu, setVcpu] = createSignal(billing()?.defaultVcpu || 2);
  const [isolated, setIsolated] = createSignal(false);
  const [presetId, setPresetId] = createSignal("");
  const preset = () => state.presets.find((x) => x.id === presetId());
  const [values, setValues] = createSignal<VariableValues>({});
  const pickPreset = (id: string) => {
    setPresetId(id);
    setValues(defaultValues(state.presets.find((x) => x.id === id)));
  };
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const estimate = (s: { vcpu: number; mem: number }) => {
    const r = rates();
    if (!r) return "";
    const micro = s.vcpu * r.rates.vcpuHourMicro + s.mem * r.rates.ramGibHourMicro + 100 * r.rates.diskGibHourMicro;
    return money(micro, r.currency) + "/h running";
  };

  const create = async () => {
    const missing = missingValue(preset(), values());
    if (missing) return setError(missing);
    setBusy(true);
    setError("");
    try {
      const size = sizes().find((s) => s.vcpu === vcpu());
      await actions.createBox(
        {
          name: name().trim() || null,
          vcpu: size?.vcpu ?? null,
          memoryBytes: size ? size.mem * GIB : null,
          isolated: isolated(),
        },
        presetId() || null,
        preset() ? values() : null,
      );
      actions.closeDialog();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="New box" onClose={actions.closeDialog}>
      <p>A persistent Linux machine on boxd. It boots in well under a second and hibernates on its own after four idle hours.</p>
      <div class="field">
        <label for="nb-name">Name</label>
        <input id="nb-name" class="input mono" placeholder="leave empty for a suggested name" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
      </div>
      <div class="field">
        <span class="label">Size</span>
        <div class="choices">
          <For each={sizes()}>
            {(s) => (
              <button class="choice" aria-pressed={vcpu() === s.vcpu} onClick={() => setVcpu(s.vcpu)}>
                <span>
                  {s.vcpu} vCPU · {s.mem} GiB
                </span>
                <span class="faint mono">{estimate(s)}</span>
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="field">
        <label for="nb-setup">Setup</label>
        <select id="nb-setup" class="input" value={presetId()} onChange={(e) => pickPreset(e.currentTarget.value)}>
          <option value="">None: an empty box</option>
          <For each={state.presets}>{(x) => <option value={x.id}>{x.name}</option>}</For>
        </select>
        <Show
          when={preset()}
          fallback={
            <p class="hint">
              <Show when={state.presets.length === 0} fallback="Pick a preset to export its secrets and run its commands in the box once it boots.">
                A setup preset runs commands and exports secrets in the box once it boots.{" "}
                <button
                  class="link"
                  onClick={() => {
                    actions.closeDialog();
                    actions.select({ kind: "presets" });
                  }}
                >
                  Write the first one
                </button>
                .
              </Show>
            </p>
          }
        >
          {(pr) => (
            <p class="hint">
              Runs {pr().commands.length} {pr().commands.length === 1 ? "command" : "commands"}
              {pr().secretNames.length > 0 ? (
                <>
                  {" "}
                  after exporting <span class="mono">{pr().secretNames.join(", ")}</span>
                </>
              ) : null}
              . Progress shows in the box's Setup tab.
            </p>
          )}
        </Show>
        <Show when={preset()}>{(pr) => <VariableFields preset={pr()} values={values()} onInput={(n, v) => setValues((vs) => ({ ...vs, [n]: v }))} />}</Show>
      </div>
      <label class="check">
        <input type="checkbox" checked={isolated()} onChange={(e) => setIsolated(e.currentTarget.checked)} />
        <span>
          <strong>Isolated sandbox.</strong> No in-VM boxd CLI, no org integrations, no stored agent logins. Cannot be changed later. Pick this for untrusted
          code.
        </span>
      </label>
      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions">
        <button class="btn" onClick={actions.closeDialog} disabled={busy()}>
          Cancel
        </button>
        <button class="btn primary" onClick={() => void create()} disabled={busy()}>
          <I.Plus /> {busy() ? "Creating…" : "Create box"}
        </button>
      </div>
    </Modal>
  );
};

const NewAgent = (p: { machineId?: string }) => {
  const [machineId, setMachineId] = createSignal(p.machineId ?? state.machines[0]?.id ?? "");
  const [harness, setHarness] = createSignal<Harness>("claude-code");
  const [cwd, setCwd] = createSignal("");
  const [model, setModel] = createSignal("");
  const [skip, setSkip] = createSignal(true);
  const [ownBranch, setOwnBranch] = createSignal(false);
  const [first, setFirst] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const create = async () => {
    const m = machineById(machineId());
    if (!m) {
      setError("Pick a box first.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      await actions.createAgent(
        {
          machineId: m.id,
          machineName: m.name,
          harness: harness(),
          cwd: cwd().trim() || null,
          model: model().trim() || null,
          skipPermissions: skip(),
          ownBranch: ownBranch() && !!cwd().trim(),
          branchHint: first().trim() || null,
        },
        first(),
      );
      actions.closeDialog();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="New agent" onClose={actions.closeDialog}>
      <div class="field">
        <label for="na-box">Box</label>
        <select id="na-box" class="input" value={machineId()} onChange={(e) => setMachineId(e.currentTarget.value)}>
          <For each={state.machines}>{(m) => <option value={m.id}>{m.name}</option>}</For>
        </select>
      </div>
      <div class="field">
        <span class="label">Harness</span>
        <div class="choices" style={{ "grid-template-columns": "1fr 1fr" }}>
          <button class="choice" aria-pressed={harness() === "claude-code"} onClick={() => setHarness("claude-code")}>
            <span>Claude Code</span>
            <span class="faint">claude -p, stream-json</span>
          </button>
          <button class="choice" aria-pressed={harness() === "codex"} onClick={() => setHarness("codex")}>
            <span>Codex</span>
            <span class="faint">codex exec --json</span>
          </button>
        </div>
      </div>
      <div class="field">
        <label for="na-cwd">Working directory</label>
        <input id="na-cwd" class="input mono" placeholder="/home/user/project (empty = home)" value={cwd()} onInput={(e) => setCwd(e.currentTarget.value)} />
      </div>
      <label class="check">
        <input type="checkbox" checked={ownBranch() && !!cwd().trim()} disabled={!cwd().trim()} onChange={(e) => setOwnBranch(e.currentTarget.checked)} />
        <span>
          <strong>Work on its own branch.</strong>{" "}
          <Show when={cwd().trim()} fallback="Needs a working directory inside a git repository.">
            A new branch <span class="mono">stowbox/…</span> in a git worktree next to the repository, in <span class="mono">&lt;repo&gt;-wt/</span>, started
            from the commit checked out in <span class="mono">{cwd().trim()}</span>. Other agents on the same repository do not touch its files; uncommitted
            changes in <span class="mono">{cwd().trim()}</span> stay there.
          </Show>
        </span>
      </label>
      <div class="field">
        <label for="na-model">Model</label>
        <input id="na-model" class="input mono" placeholder="empty = harness default" value={model()} onInput={(e) => setModel(e.currentTarget.value)} />
      </div>
      <label class="check">
        <input type="checkbox" checked={skip()} onChange={(e) => setSkip(e.currentTarget.checked)} />
        <span>
          <strong>Run tools without approval prompts.</strong> The box is a disposable VM, so this is the usual setting. Off means any tool that would ask is
          denied, since nobody can answer a prompt mid-turn.
        </span>
      </label>
      <div class="field">
        <label for="na-first">First task (optional)</label>
        <textarea id="na-first" class="input" placeholder="Add a /health endpoint and a test for it" value={first()} onInput={(e) => setFirst(e.currentTarget.value)} />
      </div>
      <p class="hint">
        The agent signs in with whatever the box has: an <span class="mono">ANTHROPIC_API_KEY</span> or <span class="mono">CLAUDE_CODE_OAUTH_TOKEN</span> secret
        set with <span class="mono">boxd env set</span>, or a login stored for your org.
      </p>
      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions">
        <button class="btn" onClick={actions.closeDialog} disabled={busy()}>
          Cancel
        </button>
        <button class="btn primary" onClick={() => void create()} disabled={busy() || !machineId()}>
          <I.Chat /> {busy() ? (ownBranch() && cwd().trim() ? "Creating the worktree…" : "Creating…") : first().trim() ? "Create and send" : "Create agent"}
        </button>
      </div>
    </Modal>
  );
};

const Settings = () => {
  const backend = () => state.provider?.secretBackend;
  return (
    <Modal title="Settings" onClose={actions.closeDialog}>
      <dl class="kv">
        <dt>Account</dt>
        <dd>{state.account?.displayName ?? "unknown"}</dd>
        <dt>Provider</dt>
        <dd>
          boxd ·{" "}
          <button class="link" onClick={() => void actions.open(state.provider?.consoleUrl ?? "https://app.boxd.sh")}>
            open console
          </button>
        </dd>
        <dt>API key</dt>
        <dd>
          <Show when={backend() === "keyring"} fallback={<span>stored in a file only your user can read, no OS keychain was available</span>}>
            stored in the OS keychain
          </Show>
        </dd>
        <dt>Theme</dt>
        <dd>
          <button class="btn sm" classList={{ primary: state.ui.theme === "dark" }} onClick={() => actions.setTheme("dark")} aria-pressed={state.ui.theme === "dark"}>
            <I.Moon /> Dark
          </button>
          <button class="btn sm" classList={{ primary: state.ui.theme === "light" }} onClick={() => actions.setTheme("light")} aria-pressed={state.ui.theme === "light"}>
            <I.Sun /> Light
          </button>
        </dd>
        <dt>Notifications</dt>
        <dd class="stack">
          <label class="check">
            <input type="checkbox" checked={state.notify.enabled} onChange={(e) => void actions.setNotify({ enabled: e.currentTarget.checked })} />
            <span>
              <strong>Notify when an agent finishes or fails.</strong> Setup runs too. Only while Stowbox is not the window in front.
            </span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              checked={state.notify.whenFocused}
              disabled={!state.notify.enabled}
              onChange={(e) => void actions.setNotify({ whenFocused: e.currentTarget.checked })}
            />
            <span>Also when Stowbox is in front</span>
          </label>
          <Show when={state.notify.enabled}>
            <Switch>
              <Match when={state.notify.permission === "denied"}>
                <p class="hint">The system blocks notifications from Stowbox. Allow them in the OS notification settings; until then nothing is shown.</p>
              </Match>
              <Match when={state.notify.permission === "default"}>
                <p class="hint">
                  Stowbox has not been allowed to notify yet.{" "}
                  <button class="link" onClick={() => void actions.allowNotifications()}>
                    Allow notifications
                  </button>
                </p>
              </Match>
              <Match when={state.notify.permission === "unsupported"}>
                <p class="hint">Notifications are not available here.</p>
              </Match>
            </Switch>
          </Show>
        </dd>
        <dt>Version</dt>
        <dd class="mono">0.1.0</dd>
      </dl>
      <p class="hint">Stowbox talks to boxd directly from this computer. No Stowbox server sees your key, your boxes or your transcripts.</p>
      <div class="dialog-actions">
        <button class="btn danger" onClick={() => void actions.signOut()}>
          <I.Key /> Forget API key
        </button>
        <button class="btn primary" onClick={actions.closeDialog}>
          Done
        </button>
      </div>
    </Modal>
  );
};

const Confirm = (p: { d: Extract<NonNullable<Dialog>, { kind: "confirm" }> }) => {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const run = async () => {
    setBusy(true);
    setError("");
    try {
      await p.d.run();
      actions.closeDialog();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal title={p.d.title} onClose={actions.closeDialog}>
      <p>{p.d.body}</p>
      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions">
        <button class="btn" onClick={actions.closeDialog} disabled={busy()}>
          Cancel
        </button>
        <button class="btn" classList={{ danger: p.d.danger, primary: !p.d.danger }} onClick={() => void run()} disabled={busy()}>
          {busy() ? "Working…" : p.d.confirmLabel}
        </button>
      </div>
    </Modal>
  );
};

/** Removing an agent that has its own worktree: offers to remove the
 * worktree too (the branch stays). git refuses a worktree with changes; the
 * dialog then shows git's words and asks before discarding them. */
const DeleteAgent = (p: { agentId: string }) => {
  const a = () => state.agents.find((x) => x.id === p.agentId);
  const working = () => !!a() && isBusy(a()!.status);
  const boxGone = () => !!a() && !machineById(a()!.machineId);
  const [remove, setRemove] = createSignal(!working() && !boxGone());
  const [refused, setRefused] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const run = async (removeWorktree: boolean, force: boolean) => {
    setBusy(true);
    setError("");
    try {
      await actions.deleteAgent(p.agentId, removeWorktree, force);
      actions.closeDialog();
    } catch (e) {
      if (removeWorktree && !force) setRefused(errorText(e));
      else setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={a()}>
      {(ag) => (
        <Modal title={`Remove "${ag().title || "this agent"}"?`} onClose={actions.closeDialog}>
          <p>The transcript is deleted from this computer.</p>
          <label class="check">
            <input type="checkbox" checked={remove()} disabled={working() || boxGone() || busy() || !!refused()} onChange={(e) => setRemove(e.currentTarget.checked)} />
            <span>
              <strong>Also remove its worktree</strong> <span class="mono">{ag().worktree!.path}</span>. The branch{" "}
              <span class="mono">{ag().worktree!.branch}</span> stays in the repository, with everything committed on it.
              <Switch>
                <Match when={working()}>
                  <span class="hint" style={{ display: "block" }}>
                    Stop the agent's turn first to remove its worktree.
                  </span>
                </Match>
                <Match when={boxGone()}>
                  <span class="hint" style={{ display: "block" }}>
                    The box is no longer in your boxd account.
                  </span>
                </Match>
              </Switch>
            </span>
          </label>
          <Show when={refused()}>
            <div class="form-error mono" style={{ "white-space": "pre-wrap" }}>
              {refused()}
            </div>
            <p class="hint">Remove it anyway to discard those changes, or keep the worktree and remove only the agent.</p>
          </Show>
          <Show when={error()}>
            <div class="form-error">{error()}</div>
          </Show>
          <div class="dialog-actions">
            <button class="btn" onClick={actions.closeDialog} disabled={busy()}>
              Cancel
            </button>
            <Show
              when={refused()}
              fallback={
                <button class="btn danger" onClick={() => void run(remove(), false)} disabled={busy()}>
                  {busy() ? "Working…" : remove() ? "Remove agent and worktree" : "Remove agent"}
                </button>
              }
            >
              <button class="btn" onClick={() => void run(false, false)} disabled={busy()}>
                Keep the worktree
              </button>
              <button class="btn danger" onClick={() => void run(true, true)} disabled={busy()}>
                {busy() ? "Working…" : "Discard changes and remove"}
              </button>
            </Show>
          </div>
        </Modal>
      )}
    </Show>
  );
};

const RunSetup = (p: { machineId: string; presetId: string }) => {
  const preset = () => state.presets.find((x) => x.id === p.presetId);
  const m = () => machineById(p.machineId);
  const [values, setValues] = createSignal<VariableValues>(defaultValues(preset()));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const run = async () => {
    const missing = missingValue(preset(), values());
    if (missing) return setError(missing);
    setBusy(true);
    setError("");
    try {
      await actions.startSetup(p.machineId, p.presetId, values());
      actions.closeDialog();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={preset()} fallback={<Modal title="Preset not found" onClose={actions.closeDialog}><p>It was deleted.</p></Modal>}>
      {(pr) => (
        <Modal title={`Run "${pr().name}"`} onClose={actions.closeDialog}>
          <p>
            Values for this run in <span class="mono">{m()?.name ?? p.machineId}</span>.
          </p>
          <VariableFields preset={pr()} values={values()} onInput={(n, v) => setValues((vs) => ({ ...vs, [n]: v }))} />
          <Show when={error()}>
            <div class="form-error">{error()}</div>
          </Show>
          <div class="dialog-actions">
            <button class="btn" onClick={actions.closeDialog} disabled={busy()}>
              Cancel
            </button>
            <button class="btn primary" onClick={() => void run()} disabled={busy()}>
              <I.Play /> {busy() ? "Starting…" : "Run preset"}
            </button>
          </div>
        </Modal>
      )}
    </Show>
  );
};

export default function Dialogs() {
  const d = () => state.ui.dialog;
  return (
    <Switch>
      <Match when={d()?.kind === "new-box"}>
        <NewBox />
      </Match>
      <Match when={d()?.kind === "run-setup"}>
        <RunSetup machineId={(d() as { machineId: string }).machineId} presetId={(d() as { presetId: string }).presetId} />
      </Match>
      <Match when={d()?.kind === "new-agent"}>
        <NewAgent machineId={(d() as { machineId?: string }).machineId} />
      </Match>
      <Match when={d()?.kind === "settings"}>
        <Settings />
      </Match>
      <Match when={d()?.kind === "delete-agent"}>
        <DeleteAgent agentId={(d() as { agentId: string }).agentId} />
      </Match>
      <Match when={d()?.kind === "confirm"}>
        <Confirm d={d() as Extract<NonNullable<Dialog>, { kind: "confirm" }>} />
      </Match>
    </Switch>
  );
}
