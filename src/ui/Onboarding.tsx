import { Show, createSignal } from "solid-js";
import { actions, state } from "../state/store";
import * as I from "../icons";
import { BrandMark } from "./common";

export default function Onboarding() {
  const [key, setKey] = createSignal("");
  const [reveal, setReveal] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const connect = async () => {
    if (!key().trim() || busy()) return;
    setBusy(true);
    setError("");
    try {
      await actions.saveKey(key());
    } catch (e) {
      setError(typeof e === "string" ? e : e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="onboarding">
      <form
        class="onboard-card"
        onSubmit={(e) => {
          e.preventDefault();
          void connect();
        }}
      >
        <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
          <BrandMark />
          <span class="brand-name">Stowbox</span>
        </div>
        <h1>Connect your boxd account</h1>
        <p>
          Stowbox drives boxd machines from your desktop: spin boxes up, run Claude Code or Codex inside them, watch the cost. The key goes straight from this
          computer to boxd; nothing passes through a Stowbox server.
        </p>
        <div class="field">
          <label for="ob-key">boxd API key</label>
          <div class="input-row">
            <input
              id="ob-key"
              class="input mono"
              type={reveal() ? "text" : "password"}
              placeholder="bxd_…"
              autocomplete="off"
              spellcheck={false}
              value={key()}
              onInput={(e) => setKey(e.currentTarget.value)}
            />
            <button type="button" class="icon-btn" title={reveal() ? "Hide" : "Show"} aria-label="Toggle key visibility" onClick={() => setReveal(!reveal())}>
              <Show when={reveal()} fallback={<I.Eye />}>
                <I.EyeSlash />
              </Show>
            </button>
          </div>
        </div>
        <p>
          Create one at{" "}
          <button type="button" class="link" onClick={() => void actions.open((state.provider?.consoleUrl ?? "https://app.boxd.sh") + "/")}>
            app.boxd.sh
          </button>{" "}
          or with <span class="mono">boxd auth keys create stowbox</span>. Keys are fenced to one organization.
        </p>
        <Show when={error()}>
          <div class="form-error">{error()}</div>
        </Show>
        <div class="modal-actions" style={{ "justify-content": "flex-start" }}>
          <button type="submit" class="btn primary" disabled={!key().trim() || busy()}>
            <I.Key /> {busy() ? "Checking with boxd…" : "Connect"}
          </button>
        </div>
      </form>
    </div>
  );
}
