import type { Api } from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const DEMO_KEY = "stowbox.demo";

function readDemo(): boolean {
  try {
    return localStorage.getItem(DEMO_KEY) === "1";
  } catch {
    return false;
  }
}

/** The app runs on the mock's example data, by the person's choice from
 * onboarding. Every screen says so; nothing reaches a provider. */
export const isDemo = readDemo();

/** Turns the demo on or off; the app reloads to swap the bridge. */
export function setDemo(on: boolean) {
  try {
    if (on) localStorage.setItem(DEMO_KEY, "1");
    else localStorage.removeItem(DEMO_KEY);
  } catch {
    // Without storage there is no demo; the reload lands on onboarding again.
  }
  window.location.replace(window.location.pathname);
}

async function load(): Promise<Api> {
  if (isTauri && !isDemo) {
    const m = await import("./tauri");
    return m.tauriApi;
  }
  // The browser preview, and the demo: example data, no desktop shell and
  // no provider. Lets the UI be developed and screenshotted without an account.
  const m = await import("./mock");
  return m.mockApi;
}

export const api: Api = await load();
