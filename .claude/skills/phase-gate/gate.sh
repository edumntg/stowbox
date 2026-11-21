#!/usr/bin/env bash
# The mechanical half of the phase gate: everything a script can decide.
# Usage: gate.sh [base-ref]   (default: the merge base with main)
# Exit 0 when every check passes; otherwise the failing checks are listed.
#
# Clippy and the secret scan count only lines changed since the base, so a
# phase answers for its own code. Everything else covers the whole tree.

set -uo pipefail
root=$(git rev-parse --show-toplevel) || exit 1
cd "$root" || exit 1
# shellcheck source=SCRIPTDIR/../../hooks/lib.sh
. .claude/hooks/lib.sh

for tool in git jq awk cargo pnpm; do
  command -v "$tool" >/dev/null 2>&1 || { echo "gate.sh needs $tool (see docs/development.md)." >&2; exit 1; }
done

# A fork may only have the upstream branch as origin/main or upstream/main.
if [ $# -gt 0 ]; then
  base=$1
else
  base=""
  for ref in main origin/main upstream/main; do
    base=$(git merge-base "$ref" HEAD 2>/dev/null) && break
  done
  [ -n "$base" ] || { echo "No main branch to compare with; pass a base ref: gate.sh <base>" >&2; exit 1; }
fi
failed=()
step() { printf '\n== %s\n' "$1"; }
fail() { failed+=("$1"); printf 'FAIL: %s\n' "$1"; }

# "file<TAB>line" for every line added or changed since the base, committed
# or not, plus every line of untracked files.
changed=$( {
  git diff -U0 "$base" -- . ':!*.lock' ':!pnpm-lock.yaml' | awk '
    /^\+\+\+ b\// { f = substr($0, 7) }
    /^@@/ { split($3, a, ","); s = substr(a[1], 2); n = (a[2] == "" ? 1 : a[2]);
            for (i = 0; i < n; i++) print f "\t" s + i }'
  git ls-files --others --exclude-standard | while read -r f; do
    [ -f "$f" ] && awk -v f="$f" '{ print f "\t" NR }' "$f"
  done
} | sort -u)

step "rustfmt"
if ! cargo fmt --all --check >/dev/null 2>&1; then
  if [ -n "${STOWBOX_FMT_BASELINE:-}" ]; then
    echo "not clean; allowed until the formatting-only commit (STOWBOX_FMT_BASELINE is set)"
  else
    fail "cargo fmt --all --check"
  fi
fi

step "clippy on changed lines (workspace lints, unsafe forbidden)"
diags=$(cargo clippy --workspace --all-targets --message-format=json -q 2>/dev/null | jq -r '
  select(.reason == "compiler-message") | .message
  | select(.level == "error" or .level == "warning")
  | (.spans[] | select(.is_primary)) as $s
  | [.level, $s.file_name, $s.line_start, $s.line_end, (.code.code // "-"), .message] | @tsv' | sort -u)
hits=$(awk -F'\t' 'NR == FNR { keep[$1 "\t" $2] = 1; next }
  $1 == "error" { print; next }
  { for (l = $3; l <= $4; l++) if (($2 "\t" l) in keep) { print; break } }' <(printf '%s\n' "$changed") <(printf '%s\n' "$diags"))
if [ -n "$hits" ]; then
  printf '%s\n' "$hits" | awk -F'\t' '{ printf "  %s %s:%s %s %s\n", $1, $2, $3, $5, $6 }'
  fail "clippy findings on changed lines"
fi

step "cargo test -p stowbox-core"
cargo test -p stowbox-core -q >/dev/null 2>&1 || fail "cargo test -p stowbox-core"

step "cargo check --workspace"
cargo check --workspace -q 2>/dev/null || fail "cargo check --workspace"

step "stowbox-core builds without Tauri"
if cargo tree -p stowbox-core -e normal --prefix none 2>/dev/null | grep -qE '^(tauri|wry|tao) '; then
  fail "stowbox-core depends on Tauri"
fi

step "pnpm build"
pnpm build >/dev/null 2>&1 || fail "pnpm build"

step "cargo audit (RustSec advisories)"
if command -v cargo-audit >/dev/null 2>&1; then
  cargo audit -q || fail "cargo audit"
else
  fail "cargo-audit is not installed: cargo install --locked cargo-audit"
fi

step "cargo deny (licences, bans, sources, advisories)"
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny --log-level error check || fail "cargo deny check"
else
  fail "cargo-deny is not installed: cargo install --locked cargo-deny"
fi

step "pnpm audit (production dependencies)"
pnpm audit --prod --audit-level high >/dev/null 2>&1 || fail "pnpm audit --prod --audit-level high"

step "secrets on changed lines"
secrets=$(printf '%s\n' "$changed" | cut -f1 | sort -u | while read -r f; do
  [ -f "$f" ] || continue
  lines=$(printf '%s\n' "$changed" | awk -F'\t' -v f="$f" '$1 == f { printf "%s ", $2 }')
  stowbox_grep_lines "$f" "$lines" "$STOWBOX_SECRET_RE" | sed "s|^|  $f:|"
done)
[ -n "$secrets" ] && { printf '%s\n' "$secrets"; fail "credential-looking strings"; }

step "provider and harness icons are inert SVG"
for svg in src/assets/providers/*.svg src/assets/harnesses/*.svg; do
  [ -f "$svg" ] || continue
  if grep -qiE '<script|<foreignObject|[[:space:]]on[a-z]+=|href="[^#]|url\(|<!ENTITY' "$svg"; then
    fail "unsafe markup in $svg"
  fi
done

printf '\n'
if [ ${#failed[@]} -gt 0 ]; then
  printf 'Gate FAILED (%d):\n' "${#failed[@]}"
  printf '  - %s\n' "${failed[@]}"
  exit 1
fi
echo "Gate passed: the mechanical checks are green. Continue with the review steps in SKILL.md."
