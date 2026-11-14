import { For, Match, Show, Switch, createMemo, createSignal, onMount } from "solid-js";
import type { HttpsRoute, Listening, ListeningPort, Machine, PortForward } from "../bridge/types";
import { api } from "../bridge/api";
import { actions, capsFor, providerLabel } from "../state/store";
import * as I from "../icons";
import { relTime, statusLabel } from "./format";

// A box's servers, reached from the browser. HTTPS links are the provider's
// proxies. On boxd: the default one at the box's own address and named ones
// at `<name>.<box address>`, and raw forwards (ExposePort) for non-HTTP
// servers. On boat: one tokened link per port, which its API cannot list, so
// only the links made from this session show. Links are read without waking
// the box; what listens inside it is read over exec, so only while it runs
// unless asked. The capabilities decide which parts show.

function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

const NAME_RE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
const MAX_FORWARDS = 3;
const SSH_PORT = 22;
const protocolLabel: Record<string, string> = { tcp: "TCP", udp: "UDP", both: "TCP and UDP" };

/** `p<port>`, then `p<port>-2`…, like core's suggest_route_name. */
function suggestName(port: number, taken: string[]): string {
  const base = `p${port}`;
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) if (!taken.includes(`${base}-${n}`)) return `${base}-${n}`;
}

const host = (url: string) => url.replace(/^https?:\/\//, "");

type Form = { kind: "link"; port: string; name: string; nameEdited: boolean } | { kind: "forward"; port: string; protocol: string };

export default function Ports(p: { machine: Machine }) {
  const m = () => p.machine;
  const caps = () => capsFor(m());
  const label = () => providerLabel(m().provider);
  const named = () => caps()?.httpsNamed ?? true;
  const canForward = () => caps()?.portForwards ?? true;
  const [routes, setRoutes] = createSignal<HttpsRoute[] | null>(null);
  const [routesError, setRoutesError] = createSignal("");
  const [forwards, setForwards] = createSignal<PortForward[] | null>(null);
  const [forwardsError, setForwardsError] = createSignal("");
  const [listening, setListening] = createSignal<Listening | null>(null);
  const [listenState, setListenState] = createSignal<"idle" | "reading" | "skipped" | "error">("idle");
  const [listenError, setListenError] = createSignal("");
  const [form, setForm] = createSignal<Form | null>(null);
  const [formError, setFormError] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [pinFor, setPinFor] = createSignal<string | null>(null);
  const [pinPort, setPinPort] = createSignal("");

  const loadRoutes = async () => {
    try {
      setRoutes(await api.httpsRoutes(m().id));
      setRoutesError("");
    } catch (e) {
      setRoutesError(message(e));
    }
  };
  const loadForwards = async () => {
    if (!canForward()) {
      setForwards([]);
      return;
    }
    try {
      setForwards(await api.portForwards(m().id));
      setForwardsError("");
    } catch (e) {
      setForwardsError(message(e));
    }
  };
  const loadListening = async (wake = false) => {
    setListenState("reading");
    try {
      const l = await api.listeningPorts(m().id, wake);
      setListening(l);
      setListenState(l ? "idle" : "skipped");
      if (wake) void actions.refreshMachines();
    } catch (e) {
      setListenError(message(e));
      setListenState("error");
    }
  };
  const loadAll = () => Promise.all([loadRoutes(), loadForwards(), loadListening(false)]);
  onMount(() => void loadAll());

  const byPort = createMemo(() => new Map((listening()?.ports ?? []).map((l) => [l.port, l])));
  /** green: something reachable listens; orange: on localhost only; grey: nothing, or not read. */
  const portSq = (port: number) => {
    const l = byPort().get(port);
    if (!listening()) return { state: "idle", title: "Not checked: the box was not read" };
    if (!l) return { state: "idle", title: `Nothing listens on ${port}` };
    if (l.localOnly) return { state: "pending", title: `Listens on ${l.addresses.join(", ")} only` };
    return { state: "busy", title: `${l.process ?? "A process"} listens on ${port}` };
  };
  const routeFor = (port: number) => (routes() ?? []).find((r) => r.port === port && r.url);
  const takenNames = () => (routes() ?? []).map((r) => r.name);

  const openLinkForm = (port?: number) => {
    setFormError("");
    setForm({ kind: "link", port: port ? String(port) : "", name: port ? suggestName(port, takenNames()) : "", nameEdited: false });
  };
  const openForwardForm = (port?: number) => {
    setFormError("");
    setForm({ kind: "forward", port: port ? String(port) : "", protocol: "tcp" });
  };
  const formPort = () => {
    const f = form();
    const n = f ? Number(f.port) : NaN;
    return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null;
  };
  const formProblem = () => {
    const f = form();
    if (!f) return "";
    if (!f.port.trim()) return "Enter the port the server listens on in the box.";
    if (formPort() === null) return "A port is a number from 1 to 65535.";
    if (f.kind === "link") {
      if (!named()) return "";
      if (!NAME_RE.test(f.name)) return "Name: lowercase letters, digits and hyphens, up to 63, not starting or ending with a hyphen.";
      if (takenNames().includes(f.name)) return `${f.name} is already a link on this box.`;
    }
    return "";
  };

  const submit = async (e: Event) => {
    e.preventDefault();
    const f = form();
    const port = formPort();
    if (!f || port === null || formProblem()) return;
    setBusy("form");
    setFormError("");
    try {
      if (f.kind === "link") {
        const r = await api.addHttpsRoute(m().id, f.name, port);
        actions.toast(r.url ? `Public link to port ${port}: ${host(r.url)}` : `Created link ${f.name} to port ${port}; ${label()} returned no address for it.`, "info");
        await loadRoutes();
      } else {
        const fw = await api.exposePort(m().id, port, f.protocol);
        actions.toast(fw.address ? `Port ${port} is public at ${fw.address}` : `Exposed port ${port}; ${label()} returned no address for it.`, "info");
        await loadForwards();
      }
      setForm(null);
    } catch (err) {
      setFormError(message(err));
    } finally {
      setBusy(null);
    }
  };

  const act = async (key: string, fn: () => Promise<unknown>, after: () => Promise<void>, fail: string) => {
    setBusy(key);
    try {
      await fn();
      await after();
    } catch (e) {
      actions.toast(`${fail}: ${message(e)}`);
    } finally {
      setBusy(null);
    }
  };
  const removeRoute = (r: HttpsRoute) => act(`route:${r.name}`, () => api.removeHttpsRoute(m().id, r.name), loadRoutes, `Could not remove ${r.name}`);
  const unexpose = (f: PortForward) => act(`fwd:${f.vmPort}`, () => api.unexposePort(m().id, f.vmPort), loadForwards, `Could not unexpose ${f.vmPort}`);
  const pin = (r: HttpsRoute, port: number | null) =>
    act(
      `route:${r.name}`,
      () => api.setDefaultRoutePort(m().id, port),
      async () => {
        setPinFor(null);
        await loadRoutes();
      },
      "Could not change the port",
    );

  const asleep = () => m().status === "suspended" || m().status === "hibernated";

  const RouteRow = (rp: { r: HttpsRoute }) => {
    const r = () => rp.r;
    const sq = () => portSq(r().port);
    const pinning = () => pinFor() === r().name;
    const pinValue = () => {
      const n = Number(pinPort());
      return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null;
    };
    return (
      <li class="ports-row">
        <span class="sq" data-state={sq().state} title={sq().title} />
        <span class="ports-main">
          <Show when={r().url} fallback={<span class="muted">{label()} returned no address{r().domain ? `: ${r().domain}` : ""}</span>}>
            {(url) => (
              <button class="link mono ports-host" title={`Open ${url()}`} onClick={() => void actions.open(url())}>
                {host(url())}
              </button>
            )}
          </Show>
          <span class="faint">{r().isDefault ? "box address" : ""}</span>
        </span>
        <span class="mono ports-target">
          <Show when={r().port} fallback={<span class="faint">port not reported</span>}>
            :{r().port}
          </Show>
          <span class="faint"> {r().portMode === "auto" ? "detected" : r().portMode === "locked" ? (r().isDefault ? "pinned" : "") : r().portMode}</span>
        </span>
        <span class="ports-actions">
          <Show when={r().url}>
            {(url) => (
              <>
                <button class="icon-btn" title="Open in the browser" aria-label={`Open ${host(url())}`} onClick={() => void actions.open(url())}>
                  <I.External />
                </button>
                <button class="icon-btn" title="Copy the link" aria-label={`Copy ${host(url())}`} onClick={() => void actions.copy(url(), "Link copied")}>
                  <I.Copy />
                </button>
              </>
            )}
          </Show>
          <Show
            when={r().isDefault && (caps()?.httpsDefaultRoute ?? true)}
            fallback={
              <Show when={caps()?.httpsRemove ?? true}>
                <button class="btn sm ghost danger" disabled={busy() === `route:${r().name}`} onClick={() => void removeRoute(r())}>
                  Remove
                </button>
              </Show>
            }
          >
            <button
              class="btn sm ghost"
              aria-expanded={pinning()}
              onClick={() => {
                setPinPort(r().port ? String(r().port) : "");
                setPinFor(pinning() ? null : r().name);
              }}
            >
              Port…
            </button>
          </Show>
        </span>
        <Show when={pinning()}>
          <div class="ports-pin">
            <label class="muted" for="ports-pin-port">
              Serve the box address from port
            </label>
            <input id="ports-pin-port" class="input mono" inputmode="numeric" value={pinPort()} onInput={(e) => setPinPort(e.currentTarget.value)} />
            <button class="btn sm" disabled={pinValue() === null || busy() !== null} onClick={() => void pin(r(), pinValue())}>
              Pin
            </button>
            <button class="btn sm ghost" disabled={busy() !== null || r().portMode === "auto"} title="Follow the first of 80, 443, 8080, 8000, 3000, 5000, 5173 that something listens on" onClick={() => void pin(r(), null)}>
              Detect it again
            </button>
          </div>
        </Show>
      </li>
    );
  };

  const ListenRow = (lp: { l: ListeningPort }) => {
    const l = () => lp.l;
    const sq = () => portSq(l().port);
    const served = () => routeFor(l().port);
    return (
      <li class="ports-row">
        <span class="sq" data-state={sq().state} title={sq().title} />
        <span class="ports-main">
          <span class="mono">:{l().port}</span>
          <span class="muted">{l().port === SSH_PORT && !l().process ? "SSH" : (l().process ?? "")}</span>
          <span class="faint mono ports-addrs">{l().addresses.join(" ")}</span>
        </span>
        <span class="ports-target">
          <Switch>
            <Match when={served()}>
              {(r) => (
                <button class="link mono ports-host" title={`Open ${r().url}`} onClick={() => void actions.open(r().url!)}>
                  {host(r().url!)}
                </button>
              )}
            </Match>
            <Match when={l().localOnly}>
              <span class="muted">localhost only, so {label()} cannot reach it</span>
            </Match>
          </Switch>
        </span>
        <span class="ports-actions">
          <Show when={!served() && !l().localOnly && l().port !== SSH_PORT}>
            <button class="btn sm" onClick={() => openLinkForm(l().port)}>
              <I.Link /> Create link
            </button>
          </Show>
        </span>
      </li>
    );
  };

  const PublicNote = (np: { port: number | null; kind: Form["kind"] }) => (
    <p class="ports-public">
      <I.Warning />
      <span>
        <strong>This is public.</strong> Anyone on the internet who has the {np.kind === "link" ? "link" : "address"} reaches whatever listens on{" "}
        {np.port ? <span class="mono">{np.port}</span> : "that port"} in {m().name}. Stowbox puts no login in front of it
        {np.kind === "forward" ? `, and ${label()} passes the traffic through without TLS` : ""}. Expose only a server you would put online as it is.
      </span>
    </p>
  );

  return (
    <div class="ports">
      <div class="setup-bar">
        <button class="btn sm" onClick={() => openLinkForm()}>
          <I.Link /> New link
        </button>
        <Show when={canForward()}>
          <button class="btn sm" disabled={(forwards()?.length ?? 0) >= MAX_FORWARDS} title={(forwards()?.length ?? 0) >= MAX_FORWARDS ? "boxd allows 3 per box: unexpose one first" : "A raw TCP or UDP port for a non-HTTP server"} onClick={() => openForwardForm()}>
            <I.Plus /> Forward a port
          </button>
        </Show>
        <span class="spacer" />
        <button class="btn sm ghost" disabled={listenState() === "reading"} onClick={() => void loadAll()}>
          <I.Refresh /> Refresh
        </button>
      </div>

      <Show when={form()}>
        {(f) => (
          <form class="ports-form" onSubmit={submit}>
            <h3 class="section-title">{f().kind === "link" ? "New HTTPS link" : "Forward a raw port"}</h3>
            <div class="ports-fields">
              <label>
                <span class="muted">Port in the box</span>
                <input
                  class="input mono"
                  inputmode="numeric"
                  value={f().port}
                  placeholder="5173"
                  autofocus
                  onInput={(e) => {
                    const port = e.currentTarget.value;
                    const cur = form();
                    if (!cur) return;
                    if (cur.kind === "link") {
                      const n = Number(port);
                      const name = cur.nameEdited ? cur.name : Number.isInteger(n) && n > 0 ? suggestName(n, takenNames()) : "";
                      setForm({ ...cur, port, name });
                    } else setForm({ ...cur, port });
                  }}
                />
              </label>
              <Show
                when={f().kind === "link" && (f() as Extract<Form, { kind: "link" }>)}
                fallback={
                  <label>
                    <span class="muted">Protocol</span>
                    <select class="input" value={(f() as Extract<Form, { kind: "forward" }>).protocol} onChange={(e) => setForm({ ...(f() as Extract<Form, { kind: "forward" }>), protocol: e.currentTarget.value })}>
                      <option value="tcp">TCP</option>
                      <option value="udp">UDP</option>
                      <option value="both">TCP and UDP</option>
                    </select>
                  </label>
                }
              >
                {(lf) => (
                  <Show when={named()}>
                    <label>
                      <span class="muted">Name, the first part of the address</span>
                      <input class="input mono" value={lf().name} placeholder="api" onInput={(e) => setForm({ ...lf(), name: e.currentTarget.value.trim(), nameEdited: true })} />
                    </label>
                  </Show>
                )}
              </Show>
            </div>
            <Show when={formPort() !== null && byPort().get(formPort()!)?.localOnly}>
              <p class="hint">
                Port <span class="mono">{formPort()}</span> listens on localhost only, so {label()} cannot reach it. Restart the server on 0.0.0.0 (for Vite,{" "}
                <span class="mono">--host</span>).
              </p>
            </Show>
            <PublicNote port={formPort()} kind={f().kind} />
            <Show when={formError()}>
              <div class="form-error">{formError()}</div>
            </Show>
            <Show when={!formError() && form()!.port.trim() && formProblem()}>
              <p class="hint">{formProblem()}</p>
            </Show>
            <div class="ports-form-actions">
              <button class="btn primary" type="submit" disabled={!!formProblem() || busy() === "form"}>
                {busy() === "form" ? "Creating…" : f().kind === "link" ? "Create public link" : "Expose publicly"}
              </button>
              <button class="btn ghost" type="button" onClick={() => setForm(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
      </Show>

      <section>
        <h3 class="section-title">HTTPS links</h3>
        <Switch>
          <Match when={routesError()}>
            <div class="form-error">Could not read the links: {routesError()}</div>
          </Match>
          <Match when={routes() === null}>
            <div class="muted">Asking {label()} for the links…</div>
          </Match>
          <Match when={routes()!.length === 0}>
            <div class="muted">
              {caps()?.httpsList ?? true ? `${label()} lists no HTTPS links for ${m().name}.` : `No links made from this Stowbox session for ${m().name}.`}
            </div>
          </Match>
          <Match when={true}>
            <ul class="ports-list">
              <For each={routes()}>{(r) => <RouteRow r={r} />}</For>
            </ul>
          </Match>
        </Switch>
        <Show
          when={caps()?.httpsDefaultRoute ?? true}
          fallback={
            <p class="note">
              {label()} serves these over HTTPS, one link per port, with a token in the address. It has no way to list or remove them, so this shows only the links
              made since Stowbox started; a stop ends them.
            </p>
          }
        >
          <p class="note">
            {label()} serves these over HTTPS. The box address follows the first of 80, 443, 8080, 8000, 3000, 5000 and 5173 that something listens on, else 8000,
            unless a port is pinned; a named link points at one port.
          </p>
        </Show>
      </section>

      <section>
        <h3 class="section-title">Listening in the box</h3>
        <Switch>
          <Match when={listenState() === "error"}>
            <div class="empty-state">
              <span>
                <strong>Could not list the ports.</strong> {listenError()}
              </span>
              <button class="btn sm" onClick={() => void loadListening(false)}>
                <I.Refresh /> Try again
              </button>
            </div>
          </Match>
          <Match when={listenState() === "reading"}>
            <div class="muted">Listing the ports in {m().name}…</div>
          </Match>
          <Match when={listenState() === "skipped" && asleep()}>
            <div class="empty-state">
              <span>
                <strong>
                  {m().name} is {m().status === "suspended" ? "in standby" : "hibernated"}.
                </strong>{" "}
                Listing its ports runs a command in the box, which wakes it and bills it as running until it idles again.
              </span>
              <button class="btn sm" onClick={() => void loadListening(true)}>
                <I.Search /> Wake and check
              </button>
            </div>
          </Match>
          <Match when={listenState() === "skipped"}>
            <div class="muted">
              {m().name} is {statusLabel[m().status].toLowerCase()}; start it to see what listens. Links and forwards stay as they are.
            </div>
          </Match>
          <Match when={listening()}>
            {(l) => (
              <Show when={l().ports.length > 0} fallback={<div class="muted">Nothing listens on a TCP port in {m().name}. Start a server, then refresh.</div>}>
                <ul class="ports-list">
                  <For each={l().ports}>{(x) => <ListenRow l={x} />}</For>
                </ul>
                <p class="note">
                  Read with <span class="mono">{l().source === "ss" ? "ss -ltnp" : "/proc/net/tcp"}</span> {relTime(l().sampledAtMs)}. A server on localhost only is not
                  reachable from a link; start it on 0.0.0.0.
                </p>
              </Show>
            )}
          </Match>
        </Switch>
      </section>

      <Show when={canForward()}>
        <section>
          <h3 class="section-title">
            Raw forwards
            <Show when={forwards()}>{(fs) => <span class="faint"> · {fs().length} of {MAX_FORWARDS}</span>}</Show>
          </h3>
          <Switch>
            <Match when={forwardsError()}>
              <div class="form-error">Could not read the forwards: {forwardsError()}</div>
            </Match>
            <Match when={forwards() === null}>
              <div class="muted">Asking {label()} for the forwards…</div>
            </Match>
            <Match when={forwards()!.length === 0}>
              <div class="muted">No raw ports are exposed on {m().name}.</div>
            </Match>
            <Match when={true}>
              <ul class="ports-list">
                <For each={forwards()}>
                  {(f) => {
                    const sq = () => portSq(f.vmPort);
                    return (
                      <li class="ports-row">
                        <span class="sq" data-state={sq().state} title={sq().title} />
                        <span class="ports-main">
                          <Show when={f.address} fallback={<span class="muted">{label()} returned no address{f.dns ? `: ${f.dns}` : ""}</span>}>
                            <span class="mono ports-host">{f.address}</span>
                          </Show>
                          <span class="tag">{protocolLabel[f.protocol] ?? f.protocol}</span>
                        </span>
                        <span class="mono ports-target">:{f.vmPort}</span>
                        <span class="ports-actions">
                          <Show when={f.address}>
                            {(a) => (
                              <button class="icon-btn" title="Copy the address" aria-label={`Copy ${a()}`} onClick={() => void actions.copy(a(), "Address copied")}>
                                <I.Copy />
                              </button>
                            )}
                          </Show>
                          <button class="btn sm ghost danger" disabled={busy() === `fwd:${f.vmPort}`} onClick={() => void unexpose(f)}>
                            Unexpose
                          </button>
                        </span>
                      </li>
                    );
                  }}
                </For>
              </ul>
            </Match>
          </Switch>
          <p class="note">For databases and other servers that do not speak HTTP. {label()} passes the traffic through untouched, with no TLS, at most three per box.</p>
        </section>
      </Show>
    </div>
  );
}
