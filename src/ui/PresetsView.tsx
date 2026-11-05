import { For, Index, Show, createMemo, createSignal, untrack } from "solid-js";
import type { Preset } from "../bridge/types";
import { presetVariables, variablesIn } from "../bridge/variables";
import { actions, state } from "../state/store";
import * as I from "../icons";
import { relTime } from "./format";

const VAR_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

interface SecretRow {
  name: string;
  value: string;
  /** A value is already stored for this name; empty input keeps it. */
  stored: boolean;
  reveal: boolean;
}

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

const Editor = (p: { preset: Preset | null; onSaved: (id: string) => void }) => {
  const [name, setName] = createSignal(p.preset?.name ?? "");
  const [commands, setCommands] = createSignal<string[]>(p.preset?.commands.length ? [...p.preset.commands] : [""]);
  const [secrets, setSecrets] = createSignal<SecretRow[]>(
    (p.preset?.secrets ?? []).map((s) => ({ name: s.name, value: "", stored: s.hasValue, reveal: false })),
  );
  // Defaults by name. Names come from the commands as they are typed, so a
  // default survives a rename that is undone and is dropped on save if unused.
  const [defaults, setDefaults] = createSignal<Record<string, string>>(Object.fromEntries((p.preset ? presetVariables(p.preset) : []).map((v) => [v.name, v.default])));
  const variables = createMemo(() => variablesIn(commands()));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const setCommand = (i: number, v: string) => setCommands((cs) => cs.map((c, j) => (j === i ? v : c)));
  const move = (i: number, d: -1 | 1) =>
    setCommands((cs) => {
      const j = i + d;
      if (j < 0 || j >= cs.length) return cs;
      const next = [...cs];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });
  const removeCommand = (i: number) => setCommands((cs) => (cs.length === 1 ? [""] : cs.filter((_, j) => j !== i)));
  const patchSecret = (i: number, patch: Partial<SecretRow>) => setSecrets((ss) => ss.map((s, j) => (j === i ? { ...s, ...patch } : s)));

  const save = async () => {
    setBusy(true);
    setError("");
    try {
      const saved = await actions.savePreset({
        id: p.preset?.id ?? null,
        name: name(),
        commands: commands(),
        secrets: secrets().map((s) => ({ name: s.name.trim(), value: s.value || null })),
        variables: variables().map((n) => ({ name: n, default: defaults()[n] ?? "" })),
      });
      setSecrets(saved.secrets.map((s) => ({ name: s.name, value: "", stored: s.hasValue, reveal: false })));
      p.onSaved(saved.id);
      actions.toast(`Saved "${saved.name}".`, "info");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="preset-editor">
      <div class="field">
        <label for="pe-name">Name</label>
        <input id="pe-name" class="input" placeholder="main commands" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
      </div>

      <section class="field">
        <span class="label">Secrets</span>
        <p class="hint">
          Written to <span class="mono">~/.config/stowbox/env.sh</span> in the box (readable only by its user) and exported before the first command.{" "}
          <span class="mono">~/.profile</span> and <span class="mono">~/.bashrc</span> load that file, so terminals and agents see the same variables. Values stay in
          this computer's keychain and are masked in the setup log.
        </p>
        <Show when={secrets().length > 0}>
          <div class="secret-rows">
            <Index each={secrets()}>
              {(s, i) => (
                <div class="secret-row">
                  <input
                    class="input mono"
                    classList={{ invalid: !!s().name && !VAR_NAME.test(s().name.trim()) }}
                    placeholder="GITHUB_TOKEN"
                    aria-label={`Secret ${i + 1} name`}
                    value={s().name}
                    onInput={(e) => patchSecret(i, { name: e.currentTarget.value })}
                  />
                  <input
                    class="input mono"
                    type={s().reveal ? "text" : "password"}
                    autocomplete="off"
                    spellcheck={false}
                    placeholder={s().stored ? "stored · type to replace" : "value"}
                    aria-label={`Value of ${s().name || `secret ${i + 1}`}`}
                    value={s().value}
                    onInput={(e) => patchSecret(i, { value: e.currentTarget.value })}
                  />
                  <button class="icon-btn" title={s().reveal ? "Hide value" : "Show what you typed"} aria-label={s().reveal ? "Hide value" : "Show value"} onClick={() => patchSecret(i, { reveal: !s().reveal })}>
                    <Show when={s().reveal} fallback={<I.Eye />}>
                      <I.EyeOff />
                    </Show>
                  </button>
                  <button class="icon-btn" title="Remove secret" aria-label={`Remove ${s().name || "secret"}`} onClick={() => setSecrets((ss) => ss.filter((_, j) => j !== i))}>
                    <I.Close />
                  </button>
                </div>
              )}
            </Index>
          </div>
        </Show>
        <div>
          <button class="btn sm" onClick={() => setSecrets((ss) => [...ss, { name: "", value: "", stored: false, reveal: false }])}>
            <I.Key /> Add secret
          </button>
        </div>
      </section>

      <section class="field">
        <span class="label">Commands</span>
        <p class="hint">
          Run in order from <span class="mono">$HOME</span>, in one bash shell with the secrets exported, so <span class="mono">cd</span> and{" "}
          <span class="mono">export</span> carry over to the next command. The first command that exits non-zero stops the run. Nothing can be typed into them:
          use non-interactive flags (<span class="mono">apt-get -y</span>).
        </p>
        <ol class="command-rows">
          <Index each={commands()}>
            {(c, i) => (
              <li class="command-row">
                <span class="step-n mono">{i + 1}</span>
                <textarea
                  class="input mono"
                  rows={Math.min(8, Math.max(1, c().split("\n").length))}
                  spellcheck={false}
                  placeholder={i === 0 ? "git clone https://$GITHUB_TOKEN@github.com/you/repo.git" : "cd repo && docker compose up -d"}
                  aria-label={`Command ${i + 1}`}
                  value={c()}
                  onInput={(e) => setCommand(i, e.currentTarget.value)}
                />
                <span class="row-tools">
                  <button class="icon-btn" title="Move up" aria-label={`Move command ${i + 1} up`} disabled={i === 0} onClick={() => move(i, -1)}>
                    <I.ArrowUp />
                  </button>
                  <button class="icon-btn" title="Move down" aria-label={`Move command ${i + 1} down`} disabled={i === commands().length - 1} onClick={() => move(i, 1)}>
                    <I.ArrowDown />
                  </button>
                  <button class="icon-btn" title="Remove command" aria-label={`Remove command ${i + 1}`} onClick={() => removeCommand(i)}>
                    <I.Close />
                  </button>
                </span>
              </li>
            )}
          </Index>
        </ol>
        <div>
          <button class="btn sm" onClick={() => setCommands((cs) => [...cs, ""])}>
            <I.Plus /> Add command
          </button>
        </div>
      </section>

      <section class="field">
        <span class="label">Variables</span>
        <p class="hint">
          Write <span class="mono">{"{{name}}"}</span> in a command to ask for a value each time the preset runs. The value goes in as one single-quoted word, so
          keep the placeholder outside quotes: <span class="mono">{"git checkout {{branch}}"}</span>. Leave the default empty to make a value required. Values are
          not secret: they show in the setup log.
        </p>
        <Show when={variables().length > 0}>
          <div class="var-rows">
            <For each={variables()}>
              {(n) => (
                <div class="var-row">
                  <label class="mono" for={`pe-var-${n}`}>
                    {n}
                  </label>
                  <input
                    id={`pe-var-${n}`}
                    class="input mono"
                    spellcheck={false}
                    autocomplete="off"
                    placeholder="no default: asked every run"
                    value={defaults()[n] ?? ""}
                    onInput={(e) => {
                      const v = e.currentTarget.value;
                      setDefaults((d) => ({ ...d, [n]: v }));
                    }}
                  />
                </div>
              )}
            </For>
          </div>
        </Show>
      </section>

      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions">
        <Show when={p.preset}>
          {(pr) => (
            <button class="btn danger" onClick={() => actions.confirmDeletePreset(pr())} disabled={busy()}>
              <I.Trash /> Delete
            </button>
          )}
        </Show>
        <span class="spacer" />
        <button class="btn primary" onClick={() => void save()} disabled={busy()}>
          <I.Check /> {busy() ? "Saving…" : p.preset ? "Save changes" : "Save preset"}
        </button>
      </div>
    </div>
  );
};

export default function PresetsView() {
  // "new" while a fresh preset is being written; otherwise a preset id.
  const [selected, setSelected] = createSignal<string | null>(state.presets[0]?.id ?? null);
  const key = createMemo(() => {
    const id = selected() ?? state.presets[0]?.id ?? "new";
    return state.presets.some((x) => x.id === id) ? id : "new";
  });
  // The editor remounts only when another preset is picked, not on saves.
  const presetFor = (k: string) => untrack(() => state.presets.find((x) => x.id === k) ?? null);

  return (
    <section class="view">
      <header class="view-head">
        <div class="view-title">
          <I.ListChecks />
          <h1>Setup presets</h1>
          <span class="meta">commands and secrets that run in a box when it is created</span>
        </div>
        <div class="view-actions">
          <button class="btn ghost" onClick={() => setSelected("new")}>
            <I.Plus /> New preset
          </button>
        </div>
      </header>
      <div class="presets">
        <nav class="preset-list scroll" aria-label="Setup presets">
          <Show when={state.presetsLoaded} fallback={<div class="side-state">Reading presets…</div>}>
            <Show when={state.presets.length > 0} fallback={<div class="side-state">No presets yet. The first one you save shows up here.</div>}>
              <ul>
                <For each={state.presets}>
                  {(p) => (
                    <li>
                      <button class="row" classList={{ selected: key() === p.id }} onClick={() => setSelected(p.id)} title={p.name}>
                        <I.ListChecks />
                        <span class="row-name">{p.name}</span>
                        <span class="row-right">
                          <span class="row-time">{relTime(p.updatedAt)}</span>
                        </span>
                        <span class="row-sub">
                          {p.commands.length} {p.commands.length === 1 ? "command" : "commands"} · {p.secretNames.length}{" "}
                          {p.secretNames.length === 1 ? "secret" : "secrets"}
                          {presetVariables(p).length > 0 ? ` · ${presetVariables(p).length} ${presetVariables(p).length === 1 ? "variable" : "variables"}` : ""}
                        </span>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </nav>
        <div class="view-body scroll">
          <Show when={key()} keyed>
            {(k) => <Editor preset={presetFor(k)} onSaved={(id) => setSelected(id)} />}
          </Show>
        </div>
      </div>
    </section>
  );
}
