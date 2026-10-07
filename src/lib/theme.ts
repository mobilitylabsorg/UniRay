// System colors (Material You on Android 12+) applied over the UI's own
// palette from styles.css.
//
// The tokens are set as inline custom properties on <html> rather than through
// an injected <style>: CSSOM changes are not subject to the page's CSP, and the
// inline values win over the stylesheet's :root rules in both themes.

import { api, type ColorTokens, type SystemColors } from "./api";

/** The tokens the system palette may replace; status colors keep their meaning. */
const TOKENS = ["bg", "surface", "raised", "line", "ink", "muted", "accent", "accent-ink", "accent-soft"];

const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");
let palette: SystemColors | null = null;

function apply() {
  const root = document.documentElement.style;
  const tokens: ColorTokens | undefined = darkQuery.matches ? palette?.dark : palette?.light;
  for (const name of TOKENS) {
    const value = tokens?.[name];
    if (value) root.setProperty(`--color-${name}`, value);
    else root.removeProperty(`--color-${name}`);
  }
}

async function refresh() {
  palette = await api.systemColors().catch(() => null);
  apply();
}

/** Starts following the system palette and theme. */
export async function followSystemColors() {
  darkQuery.addEventListener("change", apply);
  // The wallpaper, and with it the palette, may change while the app is away.
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refresh();
  });
  await refresh();
}
