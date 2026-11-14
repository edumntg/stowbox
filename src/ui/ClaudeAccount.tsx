import { Match, Show, Switch, createResource, createSignal } from "solid-js";
import { api } from "../bridge/api";
import type { ClaudeAccount, Machine } from "../bridge/types";
import { actions } from "../state/store";
import * as I from "../icons";

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Which Claude Code login the box uses, and the switch to a login of its own. */
export default function ClaudeLogin(p: { machine: Machine }) {
  // Reading runs in the box, so a box that is not running is only read on request.
  const [asked, setAsked] = createSignal(false);
  const source = () => (p.machine.status === "running" || asked() ? p.machine.id : null);
  const [account, { mutate, refetch }] = createResource(source, (id) => api.claudeAccount(id));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const set = async (own: boolean) => {
    setBusy(true);
    setError("");
    try {
      mutate(await api.setClaudeAccount(p.machine.id, own));
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const backToOrg = () =>
    actions.openDialog({
      kind: "confirm",
      title: "Use the org login again?",
      body: `Claude Code in ${p.machine.name} signs in with the org's login again, and the login stored for this box alone is deleted. Agent sessions stay.`,
      confirmLabel: "Use the org login",
      danger: true,
      run: () => set(false),
    });
  const signIn = () => actions.openBoxTerminal(p.machine.id, { kind: "claude-login" });

  const who = (a: ClaudeAccount) => {
    if (!a.source) return a.own ? "Not signed in yet." : "No login found in the box.";
    if (a.email) return a.organization ? `${a.email} · ${a.organization}` : a.email;
    return a.source === "stored login" ? "Signed in" : `Signed in with ${a.source}`;
  };

  return (
    <section class="claude-login">
      <h3 class="section-title">Claude Code login</h3>
      <Switch>
        <Match when={!source()}>
          <div class="claude-login-row">
            <span class="muted">
              {p.machine.name} is not running. Reading the login runs a command in it
              {p.machine.status === "stopped" || p.machine.status === "failed" ? ", so start it first." : ", which wakes it."}
            </span>
            <Show when={p.machine.status !== "stopped" && p.machine.status !== "failed"}>
              <button class="btn sm" onClick={() => setAsked(true)}>
                Wake and read
              </button>
            </Show>
          </div>
        </Match>
        <Match when={account.loading && !account.latest}>
          <div class="muted">Reading the login in {p.machine.name}…</div>
        </Match>
        <Match when={account.error}>
          <div class="claude-login-row">
            <span>Could not read the login: {errText(account.error)}</span>
            <button class="btn sm" onClick={() => void refetch()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Match>
        <Match when={account.latest}>
          {(a) => (
            <>
              <div class="claude-login-row">
                <span class="sq" data-state={a().source ? "ok" : "pending"} />
                <span class="claude-login-who">
                  <strong>{a().own ? "This box's own login" : "The org's login"}</strong>
                  <span class="mono">{who(a())}</span>
                </span>
                <span class="claude-login-actions">
                  <button class="btn sm" disabled={busy()} onClick={signIn} title="Runs claude /login in this box">
                    <I.Terminal /> {a().source ? "Sign in again" : "Sign in"}
                  </button>
                  <Show
                    when={a().own}
                    fallback={
                      <button class="btn sm" disabled={busy()} onClick={() => void set(true)}>
                        {busy() ? "Switching…" : "Use a separate login"}
                      </button>
                    }
                  >
                    <button class="btn sm ghost" disabled={busy()} onClick={backToOrg}>
                      Use the org login
                    </button>
                  </Show>
                  <button class="icon-btn" disabled={account.loading} onClick={() => void refetch()} title="Read again" aria-label="Read the login again">
                    <I.Refresh />
                  </button>
                </span>
              </div>
              <Show when={error()}>
                <div class="form-error">{error()}</div>
              </Show>
              <p class="note">
                {a().own ? (
                  <>
                    Kept in <span class="mono">~/.stowbox/claude</span> in this box and used by every agent here. Other boxes keep their own.
                  </>
                ) : (
                  <>
                    boxd gives every box in the org the same login. A separate login lets this box use another account, say a personal one next to a work
                    one; agents keep their sessions.
                  </>
                )}
              </p>
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}
