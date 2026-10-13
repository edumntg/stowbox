import { For } from "solid-js";
import { actions, state } from "../state/store";
import * as I from "../icons";

export default function Toasts() {
  return (
    <div class="toasts" aria-live="polite">
      <For each={state.ui.toasts}>
        {(t) => (
          <div class="toast" classList={{ error: t.kind === "error" }} role={t.kind === "error" ? "alert" : "status"}>
            <span>{t.text}</span>
            <button class="icon-btn" aria-label="Dismiss" onClick={() => actions.dismiss(t.id)}>
              <I.Close />
            </button>
          </div>
        )}
      </For>
    </div>
  );
}
