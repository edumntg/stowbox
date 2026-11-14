import type { Harness } from "../bridge/types";

/** Claude Code's built-in commands that need its interactive interface. A
 * headless turn (`claude -p`) answers them with "/x isn't available in this
 * environment", so the chat runs them in the agent's terminal instead.
 * Custom commands from `.claude/commands` and skills do work headless and
 * still go out as chat messages. */
const CLAUDE_INTERACTIVE = new Set([
  "add-dir",
  "agents",
  "bug",
  "clear",
  "config",
  "context",
  "cost",
  "doctor",
  "exit",
  "export",
  "feedback",
  "help",
  "hooks",
  "ide",
  "login",
  "logout",
  "mcp",
  "memory",
  "model",
  "output-style",
  "permissions",
  "plugin",
  "privacy-settings",
  "release-notes",
  "resume",
  "rewind",
  "settings",
  "status",
  "statusline",
  "terminal-setup",
  "theme",
  "upgrade",
  "usage",
  "vim",
]);

/** The command name of a message that is a slash command, without the slash. */
export function slashCommand(text: string): string | null {
  const m = /^\/([A-Za-z][\w:-]*)(\s|$)/.exec(text.trim());
  return m ? m[1] : null;
}

/** The message belongs in the agent's terminal, not in a headless turn.
 * Codex runs no slash command headless, so all of them go there. */
export function runsInTerminal(harness: Harness, text: string): boolean {
  const c = slashCommand(text);
  if (!c) return false;
  return harness === "codex" || CLAUDE_INTERACTIVE.has(c.toLowerCase());
}

/** The command a headless turn refused, from its answer. */
export function refusedCommand(text: string): string | null {
  const m = /(\/[A-Za-z][\w:-]*) isn['’]t available in this environment/.exec(text);
  return m ? m[1] : null;
}

/** A failed turn whose message says Claude Code is not signed in. */
export function needsLogin(text: string): boolean {
  return /\/login\b|invalid api key|not logged in|oauth token (has )?expired|authentication_error/i.test(text);
}
