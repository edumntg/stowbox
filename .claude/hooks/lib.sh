# Shared by .claude/hooks/post-edit-check.sh and
# .claude/skills/phase-gate/gate.sh. Sourced, not run. Works with bash 3.2
# (macOS) and BSD or GNU awk.
# shellcheck shell=bash disable=SC2034  # the patterns are used by the scripts that source this

# Strings that look like a live credential: cloud and provider keys, tokens,
# private keys. Placeholders in tests and docs ("dtn_test", "sk-...") are too
# short or too regular to match.
STOWBOX_SECRET_RE='(AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|sk-(ant-)?[A-Za-z0-9_-]{30,}|xox[abpr]-[A-Za-z0-9-]{20,}|dtn_[A-Za-z0-9]{30,}|e2b_[A-Za-z0-9]{30,}|-----BEGIN ([A-Z]+ )?PRIVATE KEY-----|[Bb]earer [A-Za-z0-9._~+/-]{40,})'

# Sinks that turn a string into markup or code in the webview, which can call
# every Tauri command.
STOWBOX_SINK_RE='innerHTML|outerHTML|insertAdjacentHTML|document\.write|[^A-Za-z]eval\(|new Function\(|setTimeout\(["'\'']'

# Runs "$@" with a limit of $1 seconds when timeout(1) or gtimeout(1) exists;
# macOS has neither by default, and then the command runs unbounded. Exits 124
# on timeout, like timeout(1).
stowbox_timeout() {
  local secs=$1
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "$secs" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout "$secs" "$@"
  else
    "$@"
  fi
}

# Prints "line: text" for each line of file $1 whose number is in the
# space-separated list $2 and which matches the extended regex $3. The regex
# goes through the environment because awk -v would eat its backslashes.
stowbox_grep_lines() {
  [ -n "$2" ] || return 0
  RE="$3" awk -v want="$2" '
    BEGIN { n = split(want, w, " "); for (i = 1; i <= n; i++) keep[w[i]] = 1 }
    (FNR in keep) && $0 ~ ENVIRON["RE"] { printf "%d: %s\n", FNR, $0 }' "$1"
}
