#!/usr/bin/env bash
# Claude Code PostToolUse hook for Write, Edit and MultiEdit (registered in
# .claude/settings.json). It checks the file just written and blocks, with
# exit 2 and the findings on stderr, when the file falls short of the bar in
# AGENTS.md. Most findings count only on lines that differ from HEAD: new
# code has to be clean, and old code gets cleaned when it is touched. A
# passing run may print a short JSON note for the model.
#
#   Rust        the crate builds; clippy (workspace lints, unsafe forbidden)
#               is quiet on the changed lines; rustfmt is clean if the file
#               was clean at HEAD.
#   TypeScript  tsc is quiet for the file; no raw-HTML or eval sinks on the
#               changed lines.
#   Cargo.toml  stowbox-core still builds without Tauri.
#   any file    no credential-looking strings on the changed lines.
#
# Needs bash 3.2+, git, jq and awk; cargo for Rust, pnpm for TypeScript. A
# missing tool is reported (exit 1, which does not block) rather than passed
# over in silence. Turn the hook off for yourself with "disableAllHooks" in
# .claude/settings.local.json.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=SCRIPTDIR/lib.sh
. "$here/lib.sh"

for tool in git jq awk; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "post-edit check skipped: $tool is not installed (see docs/development.md)." >&2
    exit 1
  }
done

input=$(cat)
file=$(jq -r '.tool_input.file_path // .tool_response.filePath // empty' <<<"$input")
[ -n "$file" ] && [ -f "$file" ] || exit 0

root=${CLAUDE_PROJECT_DIR:-$(git -C "$(dirname "$file")" rev-parse --show-toplevel 2>/dev/null)}
[ -n "$root" ] && cd "$root" || exit 0
case "$file" in
  "$root"/*) rel=${file#"$root"/} ;;
  *) exit 0 ;;
esac
case "$rel" in
  target/* | node_modules/* | dist/* | third-party/* | src-tauri/gen/* | *.lock | pnpm-lock.yaml) exit 0 ;;
esac

problems=()
notes=()

# The line numbers of $rel that differ from HEAD, space-separated; every
# line for a file git does not track yet.
if git ls-files --error-unmatch -- "$rel" >/dev/null 2>&1; then
  changed=$(git diff -U0 HEAD -- "$rel" | awk '/^@@/ {
    split($3, a, ","); s = substr(a[1], 2); n = (a[2] == "" ? 1 : a[2]);
    for (i = 0; i < n; i++) printf "%d ", s + i }')
else
  changed=$(awk '{ printf "%d ", NR }' "$rel")
fi

hits=$(stowbox_grep_lines "$rel" "$changed" "$STOWBOX_SECRET_RE")
[ -n "$hits" ] && problems+=("Credential-looking string on a changed line of $rel. Keys belong in the OS keychain, never in the repo:
$hits")

case "$rel" in
  *.rs)
    if ! command -v cargo >/dev/null 2>&1; then
      echo "post-edit check skipped Rust checks: cargo is not installed." >&2
      exit 1
    fi
    # The crate that owns the file: the nearest Cargo.toml above it.
    dir=$(dirname "$rel")
    while [ "$dir" != "." ] && [ ! -f "$dir/Cargo.toml" ]; do dir=$(dirname "$dir"); done
    pkg=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$dir/Cargo.toml" | head -n 1)
    [ -n "$pkg" ] || pkg=stowbox-core

    out=$(stowbox_timeout 280 cargo clippy -p "$pkg" --all-targets --message-format=json -q 2>/dev/null)
    if [ $? -eq 124 ]; then
      notes+=("clippy timed out on $pkg; run it by hand before the phase gate.")
    else
      # level, in this file?, first line, last line, lint, file, message
      diags=$(jq -r --arg f "$rel" '
        select(.reason == "compiler-message") | .message
        | select(.level == "error" or .level == "warning")
        | (.spans[] | select(.is_primary)) as $s
        | [.level, ($s.file_name == $f), $s.line_start, $s.line_end, (.code.code // "-"), $s.file_name, .message] | @tsv' <<<"$out" | sort -u)
      errors=$(awk -F'\t' '$1 == "error" { printf "%s:%s %s %s\n", $6, $3, $5, $7 }' <<<"$diags")
      [ -n "$errors" ] && problems+=("$pkg does not build:
$errors")
      own=$(awk -F'\t' -v want="$changed" '
        BEGIN { n = split(want, w, " "); for (i = 1; i <= n; i++) keep[w[i]] = 1 }
        $1 == "warning" && $2 == "true" {
          hit = 0; for (l = $3; l <= $4; l++) if (l in keep) hit = 1
          printf "%s\t%s:%s %s %s\n", (hit ? "new" : "old"), $6, $3, $5, $7 }' <<<"$diags")
      new=$(awk -F'\t' '$1 == "new" { print $2 }' <<<"$own")
      old=$(awk -F'\t' '$1 == "old" { n++ } END { print n + 0 }' <<<"$own")
      [ -n "$new" ] && problems+=("clippy warnings on lines you changed in $rel. Fix them; an #[allow] goes on one item with a comment saying why it cannot fail:
$new")
      [ "$old" -gt 0 ] && notes+=("$rel has $old older clippy warnings outside your change; clean the ones near your edit.")
    fi

    # Through stdin, rustfmt checks this file alone and not its child modules.
    if ! rustfmt --edition 2021 --config-path rustfmt.toml --check <"$rel" >/dev/null 2>&1; then
      was_clean=1
      if git cat-file -e "HEAD:$rel" 2>/dev/null; then
        git show "HEAD:$rel" | rustfmt --edition 2021 --config-path rustfmt.toml --check >/dev/null 2>&1 || was_clean=0
      fi
      if [ $was_clean -eq 1 ]; then
        problems+=("$rel is not rustfmt-clean (it was at HEAD): run rustfmt --edition 2021 $rel")
      else
        notes+=("$rel was not rustfmt-clean at HEAD either. Leave whole-file formatting to a formatting-only commit; do not reformat it inside a feature change.")
      fi
    fi
    ;;

  *.ts | *.tsx)
    if command -v pnpm >/dev/null 2>&1; then
      tsc=$(stowbox_timeout 120 pnpm exec tsc --noEmit --pretty false 2>&1 | grep -F "$rel(" | head -n 30)
      [ -n "$tsc" ] && problems+=("tsc errors in $rel:
$tsc")
    else
      notes+=("pnpm is not installed, so tsc did not run on $rel.")
    fi
    sinks=$(stowbox_grep_lines "$rel" "$changed" "$STOWBOX_SINK_RE")
    [ -n "$sinks" ] && problems+=("Raw-HTML or eval sink on a changed line of $rel. The webview can call every Tauri command; render text with JSX, or build nodes with textContent:
$sinks")
    ;;

  Cargo.toml | */Cargo.toml)
    if cargo tree -p stowbox-core -e normal --prefix none 2>/dev/null | grep -qE '^(tauri|wry|tao) '; then
      problems+=("stowbox-core now depends on Tauri; AGENTS.md says it must build without it.")
    fi
    notes+=("A manifest changed: the phase gate runs cargo audit and cargo deny on the new dependency tree.")
    ;;
esac

if [ ${#problems[@]} -gt 0 ]; then
  {
    echo "post-edit check failed for $rel:"
    for p in "${problems[@]}"; do printf '\n- %s\n' "$p"; done
    for n in ${notes[@]+"${notes[@]}"}; do printf '\n(note) %s\n' "$n"; done
  } >&2
  exit 2
fi

if [ ${#notes[@]} -gt 0 ]; then
  jq -n --arg m "post-edit check passed for $rel. ${notes[*]}" \
    '{hookSpecificOutput: {hookEventName: "PostToolUse", additionalContext: $m}}'
fi
exit 0
