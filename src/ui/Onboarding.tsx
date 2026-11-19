import { For, Show, createSignal } from "solid-js";
import type { ProviderInfo } from "../bridge/types";
import { actions, demo, state } from "../state/store";
import * as I from "../icons";
import { BrandMark, ProviderIcon } from "./common";

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

/** The host of a console URL, as the link text. */
const host = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/.*$/, "");

/** One provider's API key field, its hint and Connect. Used by onboarding
 * and by Settings for a provider that is not connected yet. */
export function ProviderKeyForm(p: { provider: ProviderInfo; onDone?: () => void }) {
  const [values, setValues] = createSignal<Record<string, string>>({});
  const [reveal, setReveal] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const caps = () => p.provider.capabilities;
  const inputId = (field: string) => `key-${p.provider.id}-${field}`;
  const complete = () => caps().credentialFields.every((f) => values()[f.name]?.trim());

  const connect = async () => {
    if (!complete() || busy()) return;
    setBusy(true);
    setError("");
    try {
      await actions.saveCredentials(p.provider.id, values());
      setValues({});
      p.onDone?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      class="key-form"
      onSubmit={(e) => {
        e.preventDefault();
        void connect();
      }}
    >
      <For each={caps().credentialFields}>
        {(f) => (
          <>
            <div class="field">
              <label for={inputId(f.name)}>
                {caps().label} {f.label}
              </label>
              <div class="input-row">
                <input
                  id={inputId(f.name)}
                  class="input mono"
                  type={f.secret && !reveal() ? "password" : "text"}
                  placeholder={f.name === "api_key" ? `${caps().keyPrefixes[0] ?? ""}…` : ""}
                  autocomplete="off"
                  spellcheck={false}
                  value={values()[f.name] ?? ""}
                  onInput={(e) => setValues({ ...values(), [f.name]: e.currentTarget.value })}
                />
                <Show when={f.secret}>
                  <button type="button" class="icon-btn" title={reveal() ? "Hide" : "Show"} aria-label="Toggle key visibility" onClick={() => setReveal(!reveal())}>
                    <Show when={reveal()} fallback={<I.Eye />}>
                      <I.EyeOff />
                    </Show>
                  </button>
                </Show>
              </div>
            </div>
            <p class="hint">
              {f.hint}{" "}
              <button type="button" class="link" onClick={() => void actions.open(caps().consoleUrl)}>
                {host(caps().consoleUrl)}
              </button>
            </p>
          </>
        )}
      </For>
      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions" style={{ "justify-content": "flex-start" }}>
        <button type="submit" class="btn primary" disabled={!complete() || busy()}>
          <I.Key /> {busy() ? `Checking with ${caps().label}…` : `Connect ${caps().label}`}
        </button>
      </div>
    </form>
  );
}

export default function Onboarding() {
  const providers = () => state.providers?.providers ?? [];
  const [pick, setPick] = createSignal("");
  const chosen = () => providers().find((p) => p.id === pick()) ?? providers()[0];

  return (
    <div class="onboarding">
      <div class="onboard-card">
        <div style={{ display: "flex", "align-items": "center", gap: "8px", "font-size": "var(--text-xs)", "font-weight": "500" }}>
          <BrandMark size={16} />
          <span>Stowbox</span>
        </div>
        <h1>Connect a cloud box provider</h1>
        <p>
          Stowbox drives cloud machines from your desktop: spin boxes up, run Claude Code or Codex inside them, watch the cost. The key goes straight from this
          computer to the provider; nothing passes through a Stowbox server. You can connect the others later from Settings and use them side by side.
        </p>
        <div class="choices" role="group" aria-label="Provider">
          <For each={providers()}>
            {(p) => (
              <button type="button" class="choice" aria-pressed={chosen()?.id === p.id} onClick={() => setPick(p.id)}>
                <span class="choice-name">
                  <ProviderIcon provider={p.id} /> {p.capabilities.label}
                </span>
                <span class="faint">{p.capabilities.currency.toUpperCase()} · {p.capabilities.customSize ? "any size" : `${p.capabilities.sizes.length} sizes`}</span>
              </button>
            )}
          </For>
        </div>
        <Show when={chosen()} keyed>
          {(p) => <ProviderKeyForm provider={p} />}
        </Show>
        <div class="onboard-demo">
          <Show
            when={!demo}
            fallback={
              <>
                <span>This is the demo, with example data.</span>
                <button type="button" class="btn sm" onClick={() => actions.exitDemo()}>
                  Leave the demo
                </button>
              </>
            }
          >
            <span>No key yet? Look around first, with example boxes and agents.</span>
            <button type="button" class="btn sm" onClick={() => actions.startDemo()}>
              <I.Play /> Try the demo
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}
