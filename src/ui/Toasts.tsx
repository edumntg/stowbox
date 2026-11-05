import { For, Show, createSignal, onCleanup } from "solid-js";
import { actions, state } from "../state/store";
import * as I from "../icons";

export default function Toasts() {
  // Only countdown toasts need the clock; a second is fine-grained enough.
  const [now, setNow] = createSignal(Date.now());
  const tick = window.setInterval(() => {
    if (state.ui.toasts.some((t) => t.untilMs)) setNow(Date.now());
  }, 1000);
  onCleanup(() => window.clearInterval(tick));
  const left = (until: number) => Math.max(0, Math.ceil((until - now()) / 1000));
  return (
    <div class="toasts" aria-live="polite">
      <For each={state.ui.toasts}>
        {(t) => (
          <div class="toast" classList={{ error: t.kind === "error" }} role={t.kind === "error" ? "alert" : "status"}>
            <span class="toast-text">
              <span>
                {t.text}
                <Show when={t.untilMs}>{(u) => <span class="mono toast-count"> in {left(u())} s</span>}</Show>
              </span>
              <Show when={t.detail}>
                <span class="toast-detail">{t.detail}</span>
              </Show>
            </span>
            <Show when={t.action}>
              {(a) => (
                <button class="btn sm" onClick={() => a().run()}>
                  {a().label}
                </button>
              )}
            </Show>
            <Show when={!t.action}>
              <button class="icon-btn" aria-label="Dismiss" onClick={() => actions.dismiss(t.id)}>
                <I.Close />
              </button>
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}
