import { For, Show, createSignal } from "solid-js";
import type { ProviderInfo } from "../bridge/types";
import { actions, state } from "../state/store";
import * as I from "../icons";
import { BrandMark } from "./common";

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

/** The host of a console URL, as the link text. */
const host = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/.*$/, "");

/** One provider's API key field, its hint and Connect. Used by onboarding
 * and by Settings for a provider that is not connected yet. */
export function ProviderKeyForm(p: { provider: ProviderInfo; onDone?: () => void }) {
  const [key, setKey] = createSignal("");
  const [reveal, setReveal] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const caps = () => p.provider.capabilities;
  const inputId = () => `key-${p.provider.id}`;

  const connect = async () => {
    if (!key().trim() || busy()) return;
    setBusy(true);
    setError("");
    try {
      await actions.saveKey(p.provider.id, key());
      setKey("");
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
      <div class="field">
        <label for={inputId()}>{caps().label} API key</label>
        <div class="input-row">
          <input
            id={inputId()}
            class="input mono"
            type={reveal() ? "text" : "password"}
            placeholder={`${caps().keyPrefixes[0] ?? ""}…`}
            autocomplete="off"
            spellcheck={false}
            value={key()}
            onInput={(e) => setKey(e.currentTarget.value)}
          />
          <button type="button" class="icon-btn" title={reveal() ? "Hide" : "Show"} aria-label="Toggle key visibility" onClick={() => setReveal(!reveal())}>
            <Show when={reveal()} fallback={<I.Eye />}>
              <I.EyeOff />
            </Show>
          </button>
        </div>
      </div>
      <p class="hint">
        {caps().keyHint}{" "}
        <button type="button" class="link" onClick={() => void actions.open(caps().consoleUrl)}>
          {host(caps().consoleUrl)}
        </button>
      </p>
      <Show when={error()}>
        <div class="form-error">{error()}</div>
      </Show>
      <div class="dialog-actions" style={{ "justify-content": "flex-start" }}>
        <button type="submit" class="btn primary" disabled={!key().trim() || busy()}>
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
          computer to the provider; nothing passes through a Stowbox server. You can connect the other provider later from Settings and use both.
        </p>
        <div class="choices two" role="group" aria-label="Provider">
          <For each={providers()}>
            {(p) => (
              <button type="button" class="choice" aria-pressed={chosen()?.id === p.id} onClick={() => setPick(p.id)}>
                <span>{p.capabilities.label}</span>
                <span class="faint">{p.capabilities.currency.toUpperCase()} · {p.capabilities.customSize ? "any size" : `${p.capabilities.sizes.length} sizes`}</span>
              </button>
            )}
          </For>
        </div>
        <Show when={chosen()} keyed>
          {(p) => <ProviderKeyForm provider={p} />}
        </Show>
      </div>
    </div>
  );
}
