import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onMount } from "solid-js";
import type { GitDiffFile, GitReview, Machine } from "../bridge/types";
import { api } from "../bridge/api";
import { actions } from "../state/store";
import * as I from "../icons";
import { PowerActions } from "./common";
import { patchLines } from "./diff";
import { relTime, statusLabel } from "./format";

// The repository an agent works in, read inside the box: branch, changed
// files and the full diff, with commit, push and pull request actions.
// Everything goes over exec; nothing reads a box that is not running unless
// the person asks, because exec wakes standby and hibernated boxes.

function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

const plural = (n: number, one: string, many = one + "s") => `${n} ${n === 1 ? one : many}`;

/** Files with more lines than this start folded, so a lockfile does not bury the rest. */
const FOLD_LINES = 400;

const FileDiff = (p: { file: GitDiffFile; index: number }) => {
  const lines = createMemo(() => patchLines(p.file.patch));
  const [open, setOpen] = createSignal(lines().length <= FOLD_LINES && p.index < 60);
  return (
    <details class="review-file" id={`review-file-${p.index}`} open={open()} onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary>
        <I.ChevronRight class="caret" />
        <span class="mono review-path" title={p.file.path}>
          <Show when={p.file.oldPath}>
            <span class="faint">{p.file.oldPath} → </span>
          </Show>
          {p.file.path}
        </span>
        <span class="faint">{p.file.change === "modified" ? "" : p.file.change}</span>
        <span class="mono review-counts">
          <span class="plus">+{p.file.additions}</span> <span class="minus">−{p.file.deletions}</span>
        </span>
      </summary>
      {/* Lines are only in the DOM while the file is open. */}
      <Show when={open()}>
        <Show
          when={p.file.patch}
          fallback={<div class="review-nopatch">{p.file.binary ? "Binary file, not shown." : p.file.change === "renamed" ? "Renamed without changes." : "No text changes (mode only)."}</div>}
        >
          <div class="diff">
            <For each={lines()}>{(l) => <div class={l.kind}>{l.text}</div>}</For>
          </div>
        </Show>
      </Show>
    </details>
  );
};

type Action = "commit" | "push" | "pr";
type Outcome = { kind: "ok" | "error"; title: string; text: string; url?: string };

const Review = (p: { review: GitReview; machine: Machine; cwd: string | null; refreshing: boolean; reload: () => Promise<void> }) => {
  const r = () => p.review;
  const [msg, setMsg] = createSignal("");
  const [busy, setBusy] = createSignal<Action | null>(null);
  const [outcome, setOutcome] = createSignal<Outcome | null>(null);
  const byPath = createMemo(() => new Map(r().diff.map((f, i) => [f.path, { a: f.additions, d: f.deletions, i }])));
  const totals = createMemo(() => r().diff.reduce((t, f) => ({ a: t.a + f.additions, d: t.d + f.deletions }), { a: 0, d: 0 }));
  const b = () => r().branch;

  const run = async (what: Action, fn: () => Promise<Outcome>) => {
    setBusy(what);
    setOutcome(null);
    try {
      setOutcome(await fn());
      await p.reload();
    } catch (e) {
      const text = message(e);
      const title =
        what === "commit" ? "Commit failed" : what === "push" ? (/\brejected\b/.test(text) ? "Push rejected" : "Push failed") : "Could not open the pull request";
      setOutcome({ kind: "error", title, text });
    } finally {
      setBusy(null);
    }
  };

  const commit = () =>
    run("commit", async () => {
      const out = await api.gitCommit(p.machine.id, p.cwd, msg());
      setMsg("");
      return { kind: "ok", title: "Committed", text: out };
    });
  const push = () =>
    run("push", async () => {
      const out = await api.gitPush(p.machine.id, p.cwd);
      return { kind: "ok", title: b().upstream ? "Pushed" : "Pushed and tracking the remote branch", text: out };
    });
  const openPr = () =>
    run("pr", async () => {
      const pr = await api.gitOpenPr(p.machine.id, p.cwd);
      await actions.open(pr.url);
      const title = pr.via === "compare" ? "Compare page opened in the browser" : pr.existing ? "A pull request for this branch is already open" : "Pull request created";
      return { kind: "ok", title, text: "", url: pr.url };
    });

  const pushBlocked = () => {
    if (!b().head) return "HEAD is detached: check out a branch to push it";
    if (!r().remote) return "This repository has no remote";
    if (b().upstream && b().ahead === 0) return "Nothing to push";
    return "";
  };
  const prBlocked = () => {
    if (!b().head) return "HEAD is detached";
    if (!r().remote) return "This repository has no remote";
    if (!b().upstream) return "Push the branch first";
    return "";
  };
  const prHow = () =>
    r().gh && r().token
      ? "Pull requests open with gh in the box, signed in with the token from the environment."
      : r().gh
        ? "Pull requests open with gh in the box when it is signed in; otherwise the compare page opens in the browser."
        : "No gh in the box: Open pull request takes you to the compare page in the browser.";

  return (
    <div class="review">
      <div class="review-head">
        <Show when={b().head} fallback={<span class="tag">detached at {b().oid?.slice(0, 7) ?? "?"}</span>}>
          <strong class="mono">{b().head}</strong>
        </Show>
        <span class="muted">
          <Show when={b().upstream} fallback={<Show when={b().head}>not pushed yet</Show>}>
            {b().ahead === 0 && b().behind === 0 ? "up to date with " : `${b().ahead} ahead, ${b().behind} behind `}
            <span class="mono">{b().upstream}</span>
          </Show>
        </span>
        <span class="faint mono review-root" title={r().root}>
          {r().root}
        </span>
        <span class="spacer" />
        <span class="faint">read {relTime(r().sampledAtMs)}</span>
        <button class="btn sm ghost" disabled={p.refreshing || !!busy()} onClick={() => void p.reload()} title="Read the repository again">
          <I.Refresh /> {p.refreshing ? "Reading…" : "Refresh"}
        </button>
      </div>

      <div class="review-actions">
        <input
          class="input mono"
          placeholder={r().files.length ? "Commit message" : "No changes to commit"}
          aria-label="Commit message"
          value={msg()}
          disabled={!r().files.length || !!busy()}
          onInput={(e) => setMsg(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && msg().trim() && !busy()) void commit();
          }}
        />
        <button class="btn primary" disabled={!r().files.length || !msg().trim() || !!busy()} title="Stage every change (git add -A) and commit" onClick={() => void commit()}>
          <I.Check /> {busy() === "commit" ? "Committing…" : "Commit all"}
        </button>
        <button class="btn" disabled={!!pushBlocked() || !!busy()} title={pushBlocked() || (b().upstream ? `git push to ${b().upstream}` : `git push -u ${r().remote} ${b().head}`)} onClick={() => void push()}>
          <I.ArrowUp /> {busy() === "push" ? "Pushing…" : "Push"}
        </button>
        <button class="btn" disabled={!!prBlocked() || !!busy()} title={prBlocked() || prHow()} onClick={() => void openPr()}>
          <I.External /> {busy() === "pr" ? "Opening…" : "Open pull request"}
        </button>
      </div>
      <p class="note">{prHow()}</p>

      <Show when={outcome()}>
        {(o) => (
          <div class={o().kind === "error" ? "review-outcome error" : "review-outcome"} role={o().kind === "error" ? "alert" : "status"}>
            <div class="review-outcome-title">
              <span class="sq" data-state={o().kind === "error" ? "failed" : "ok"} />
              <strong>{o().title}</strong>
              <Show when={o().url}>
                {(u) => (
                  <button class="link mono" onClick={() => void actions.open(u())}>
                    {u()}
                  </button>
                )}
              </Show>
              <span class="spacer" />
              <button class="icon-btn" aria-label="Dismiss" onClick={() => setOutcome(null)}>
                <I.Close />
              </button>
            </div>
            <Show when={o().text}>
              <pre class="mono">{o().text}</pre>
            </Show>
          </div>
        )}
      </Show>

      <Show
        when={r().files.length > 0}
        fallback={
          <div class="empty-state">
            <span>
              <strong>No changes.</strong> The working tree matches the last commit
              {b().head ? ` on ${b().head}` : ""}.
              {b().upstream && b().ahead > 0 ? ` ${plural(b().ahead, "commit")} not pushed yet.` : ""}
              {!b().upstream && b().head && b().oid ? " The branch is not on the remote yet." : ""}
            </span>
          </div>
        }
      >
        <section>
          <h3 class="section-title">
            {plural(r().files.length, "changed file")}
            {r().filesTruncated ? " (list cut)" : ""} · <span class="plus">+{totals().a}</span> <span class="minus">−{totals().d}</span>
          </h3>
          <ul class="review-files">
            <For each={r().files}>
              {(f) => {
                const c = () => byPath().get(f.path);
                return (
                  <li>
                    <button
                      class="review-file-row"
                      disabled={!c()}
                      onClick={() => document.getElementById(`review-file-${c()!.i}`)?.scrollIntoView({ block: "start", behavior: "smooth" })}
                      title={f.staged ? "Partly or fully staged" : "Not staged"}
                    >
                      <span class="mono review-code" title={f.code === "?" ? "untracked" : f.code === "U" ? "conflict" : undefined}>
                        {f.code}
                      </span>
                      <span class="mono review-path">
                        <Show when={f.origPath}>
                          <span class="faint">{f.origPath} → </span>
                        </Show>
                        {f.path}
                      </span>
                      <Show when={c()}>
                        {(n) => (
                          <span class="mono review-counts">
                            <span class="plus">+{n().a}</span> <span class="minus">−{n().d}</span>
                          </span>
                        )}
                      </Show>
                    </button>
                  </li>
                );
              }}
            </For>
          </ul>
          <Show when={r().filesTruncated}>
            <p class="note">The file list was cut at 256 KB of git status output. The terminal has the rest.</p>
          </Show>
        </section>

        <section class="review-diff">
          <For each={r().diff}>{(f, i) => <FileDiff file={f} index={i()} />}</For>
          <Show when={r().diffTruncated}>
            <p class="note">
              The diff is cut at 512 KB, so the last file shown may end early and later files are missing. Run <span class="mono">git diff</span> in the
              Terminal tab for all of it.
            </p>
          </Show>
        </section>
      </Show>
    </div>
  );
};

/**
 * The Changes tab. `cwd` null is the home directory. With `editable`, the
 * person can point it at another directory (the box's tab). `agentBusy`
 * reloads the review when a running turn ends.
 */
export default function Changes(p: { machine: Machine | undefined; cwd: string | null; editable?: boolean; suggestions?: string[]; agentBusy?: boolean }) {
  const [cwd, setCwd] = createSignal<string | null>(p.cwd);
  const [draft, setDraft] = createSignal(p.cwd ?? "~");
  const [review, setReview] = createSignal<GitReview | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const m = () => p.machine;
  const awake = () => m()?.status === "running";

  let seq = 0;
  const load = async () => {
    const machine = m();
    if (!machine) return;
    const mine = ++seq;
    setLoading(true);
    setError("");
    try {
      const r = await api.gitReview(machine.id, cwd());
      if (mine === seq) setReview(r);
    } catch (e) {
      if (mine === seq) setError(message(e));
    } finally {
      if (mine === seq) setLoading(false);
    }
  };

  onMount(() => {
    if (awake()) void load();
  });
  // A turn that ends usually changed files: read again.
  createEffect(
    on(
      () => p.agentBusy,
      (now, before) => {
        if (before && !now && awake()) void load();
      },
      { defer: true },
    ),
  );

  const choose = () => {
    const v = draft().trim();
    setCwd(v && v !== "~" ? v : null);
    setReview(null);
    void load();
  };

  return (
    <div class="changes">
      <Show when={p.editable}>
        <div class="setup-bar">
          <label class="muted" for="changes-cwd">
            Repository
          </label>
          <input
            id="changes-cwd"
            class="input mono"
            list="changes-cwd-list"
            value={draft()}
            placeholder="~/project"
            onInput={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && choose()}
          />
          <datalist id="changes-cwd-list">
            <For each={p.suggestions ?? []}>{(s) => <option value={s} />}</For>
          </datalist>
          <button class="btn" disabled={loading() || !m() || m()!.status === "stopped"} onClick={choose}>
            <I.FileDiff /> Read
          </button>
        </div>
      </Show>

      <Switch>
        <Match when={!m()}>
          <div class="empty-state">
            <span>
              <strong>This box no longer exists in boxd.</strong> Its repository went with it.
            </span>
          </div>
        </Match>
        <Match when={review() && review()!.state === "ok" && review()}>
          {(r) => (
            <>
              <Show when={error()}>
                <div class="form-error">Could not read the repository again: {error()}</div>
              </Show>
              <Review review={r()} machine={m()!} cwd={cwd()} refreshing={loading()} reload={load} />
            </>
          )}
        </Match>
        <Match when={review()}>
          {(r) => (
            <div class="empty-state">
              <span>
                <Switch>
                  <Match when={r().state === "not-repo"}>
                    <strong>
                      <span class="mono">{cwd() ?? "~"}</span> is not inside a git repository.
                    </strong>{" "}
                    {p.editable ? "Point this at the directory the project was cloned into." : "The agent works there, so there is nothing to review yet."}
                  </Match>
                  <Match when={r().state === "no-dir"}>
                    <strong>
                      <span class="mono">{cwd() ?? "~"}</span> does not exist in {m()!.name}.
                    </strong>{" "}
                    It may not have been cloned yet.
                  </Match>
                  <Match when={r().state === "no-git"}>
                    <strong>git is not installed in {m()!.name}.</strong> Install it from the Terminal tab.
                  </Match>
                </Switch>
              </span>
              <button class="btn sm" disabled={loading()} onClick={() => void load()}>
                <I.Refresh /> {loading() ? "Reading…" : "Read again"}
              </button>
            </div>
          )}
        </Match>
        <Match when={error()}>
          <div class="empty-state">
            <span>
              <strong>Could not read the repository.</strong> {error()}
            </span>
            <button class="btn sm" disabled={loading()} onClick={() => void load()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Match>
        <Match when={loading()}>
          <div class="muted">
            Reading <span class="mono">{cwd() ?? "~"}</span> in {m()!.name}…
          </div>
        </Match>
        <Match when={m()!.status === "suspended" || m()!.status === "hibernated"}>
          <div class="empty-state">
            <span>
              <strong>
                {m()!.name} is {m()!.status === "suspended" ? "in standby" : "hibernated"}.
              </strong>{" "}
              Reading the repository runs git in the box, which wakes it and bills it as running until it idles again.
            </span>
            <button class="btn sm" onClick={() => void load()}>
              <I.FileDiff /> Wake and read
            </button>
          </div>
        </Match>
        <Match when={!awake()}>
          <div class="empty-state">
            <span>
              <strong>
                {m()!.name} is {statusLabel[m()!.status].toLowerCase()}.
              </strong>{" "}
              Start it to read its repository.
            </span>
            <div style={{ display: "flex", gap: "4px" }}>
              <PowerActions machine={m()!} />
            </div>
          </div>
        </Match>
        <Match when={true}>
          <div class="empty-state">
            <button class="btn sm" onClick={() => void load()}>
              <I.FileDiff /> Read the repository
            </button>
          </div>
        </Match>
      </Switch>
    </div>
  );
}
