import { Match, Show, Switch } from "solid-js";
import { actions, selectedAgent, selectedMachine, state } from "../state/store";
import * as I from "../icons";
import AgentView from "./AgentView";
import BoxView from "./BoxView";
import Dialogs from "./Dialogs";
import Inspector from "./Inspector";
import Onboarding from "./Onboarding";
import Sidebar from "./Sidebar";
import Toasts from "./Toasts";

const EmptyMain = () => (
  <div class="empty-main">
    <h2>Nothing selected</h2>
    <span>Pick a box or an agent on the left. Boxes are persistent VMs; agents are Claude Code or Codex sessions running inside them.</span>
    <div class="actions">
      <button class="btn" onClick={() => actions.openDialog({ kind: "new-box" })}>
        <I.Plus /> New box
      </button>
      <button class="btn primary" disabled={state.machines.length === 0} onClick={() => actions.openDialog({ kind: "new-agent" })}>
        <I.Chat /> New agent
      </button>
    </div>
  </div>
);

const Shell = () => {
  const agent = selectedAgent;
  const machine = selectedMachine;
  const inspector = () => state.ui.inspector && !!agent();
  return (
    <div class="shell" classList={{ "has-inspector": inspector() }}>
      <Sidebar />
      <main class="main">
        <Switch fallback={<EmptyMain />}>
          <Match when={agent()}>{(a) => <AgentView agent={a()} />}</Match>
          <Match when={machine()}>{(m) => <BoxView machine={m()} />}</Match>
        </Switch>
      </main>
      <Show when={inspector()}>
        <Inspector agent={agent()!} />
      </Show>
    </div>
  );
};

export default function App() {
  return (
    <>
      <Switch>
        <Match when={state.phase === "booting"}>
          <div class="boot">Starting…</div>
        </Match>
        <Match when={state.phase === "needs-key"}>
          <Onboarding />
        </Match>
        <Match when={state.phase === "error"}>
          <div class="boot boot-error">
            <span>Stowbox could not start: {state.bootError}</span>
            <button class="btn" onClick={() => void actions.boot()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Match>
        <Match when={state.phase === "ready"}>
          <Shell />
        </Match>
      </Switch>
      <Dialogs />
      <Toasts />
    </>
  );
}
