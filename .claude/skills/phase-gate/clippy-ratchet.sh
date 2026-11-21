#!/usr/bin/env bash
# Per-file ceiling on the safety lints AGENTS.md asks for (the
# [workspace.lints.clippy] list in Cargo.toml). Older code still has some, so
# each file may keep at most the count recorded in clippy-baseline.tsv, and
# that count only goes down: more fails, and fewer fails until the baseline
# is lowered with --update in the same change.
#
# Usage: clippy-ratchet.sh [--update] [clippy-json]
#   clippy-json  output of `cargo clippy --workspace --all-targets
#                --message-format=json`; run here when omitted.
#   --update     rewrite the baseline to the current counts. It refuses to
#                raise any file's count.
#
# Only these lints count, because they do not change between clippy
# releases the way the default groups do; the changed-line check in gate.sh
# and the post-edit hook cover the rest.

set -uo pipefail
root=$(git rev-parse --show-toplevel) || exit 1
cd "$root" || exit 1
baseline=.claude/skills/phase-gate/clippy-baseline.tsv
lints='clippy::unwrap_used clippy::expect_used clippy::panic clippy::todo clippy::unimplemented clippy::dbg_macro clippy::indexing_slicing clippy::string_slice'

update=0
if [ "${1:-}" = "--update" ]; then
  update=1
  shift
fi
command -v jq >/dev/null 2>&1 || { echo "clippy-ratchet.sh needs jq." >&2; exit 1; }

tmp=$(mktemp -d) || exit 1
trap 'rm -rf "$tmp"' EXIT
if [ $# -gt 0 ]; then
  json=$1
else
  json=$tmp/clippy.json
  # Exit status matters: without clippy, or on a build error, the output
  # would be empty and every count would read as zero.
  if ! cargo clippy --workspace --all-targets --message-format=json -q >"$json" 2>"$tmp/stderr"; then
    cat "$tmp/stderr" >&2
    echo "cargo clippy failed; the ratchet cannot count." >&2
    exit 1
  fi
fi

# "file<TAB>count", one row per file with at least one hit. A warning seen
# by both the lib and the test build is counted once.
jq -r --arg lints "$lints" '
  ($lints | split(" ")) as $want
  | select(.reason == "compiler-message") | .message
  | select(.level == "warning")
  | select((.code.code // "") as $c | $want | any(.[]; . == $c))
  | (.spans[] | select(.is_primary)) as $s
  | [$s.file_name, $s.line_start, $s.column_start, .code.code] | @tsv' "$json" |
  sort -u | cut -f1 | sort | uniq -c | awk '{ printf "%s\t%d\n", $2, $1 }' >"$tmp/now"

[ -f "$baseline" ] || : >"$baseline"
# "file<TAB>baseline<TAB>now" for every file in either list.
awk -F'\t' 'NR == FNR { b[$1] = $2; next } { n[$1] = $2 }
  END { for (f in b) print f "\t" b[f] "\t" (f in n ? n[f] : 0)
        for (f in n) if (!(f in b)) print f "\t0\t" n[f] }' \
  <(grep -v '^#' "$baseline") "$tmp/now" | sort >"$tmp/cmp"

up=$(awk -F'\t' '$3 > $2 { printf "  %s: %d, was %d\n", $1, $3, $2 }' "$tmp/cmp")
down=$(awk -F'\t' '$3 < $2 { printf "  %s: %d, was %d\n", $1, $3, $2 }' "$tmp/cmp")

if [ -n "$up" ]; then
  echo "More safety-lint warnings than the baseline allows (unwrap, expect, panic, indexing, slicing...):"
  printf '%s\n' "$up"
  echo "Fix the new ones; the baseline never goes up."
  exit 1
fi
if [ $update -eq 1 ]; then
  {
    echo "# Safety-lint warnings allowed per file; written by clippy-ratchet.sh --update."
    cat "$tmp/now"
  } >"$baseline"
  echo "Baseline written: $(awk -F'\t' '{ s += $2 } END { print s + 0 }' "$tmp/now") warnings in $(wc -l <"$tmp/now" | tr -d ' ') files."
  exit 0
fi
if [ -n "$down" ]; then
  echo "Fewer safety-lint warnings than the baseline records:"
  printf '%s\n' "$down"
  echo "Lower it in this change: .claude/skills/phase-gate/clippy-ratchet.sh --update"
  exit 1
fi
echo "Safety lints at the baseline: $(awk -F'\t' '{ s += $2 } END { print s + 0 }' "$tmp/now") warnings."
