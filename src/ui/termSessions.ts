/** Terminal sessions that outlive their view: an agent's Terminal tab and a
 * box's shell keep running while you look at something else, and the view
 * picks them up again when it comes back. Kept apart from Terminal.tsx so
 * the store can close them without loading xterm. */

export interface Closable {
  close(): void;
}

export const sessions = new Map<string, Closable>();

/** Ends the session kept under `key` (the process in the box too), if any. */
export function closeTerminal(key: string) {
  sessions.get(key)?.close();
}

export const agentTerminalKey = (agentId: string) => `agent:${agentId}`;
export const boxTerminalKey = (machineId: string) => `box:${machineId}`;
