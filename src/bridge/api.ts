import type { Api } from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function load(): Promise<Api> {
  if (isTauri) {
    const m = await import("./tauri");
    return m.tauriApi;
  }
  // Browser preview only: lets the UI be developed and screenshotted without
  // the desktop shell. Never bundled into the Tauri build path.
  const m = await import("./mock");
  return m.mockApi;
}

export const api: Api = await load();
