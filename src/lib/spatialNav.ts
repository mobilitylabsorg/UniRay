// Moving the focus with a TV remote's D-pad: the arrow keys go to the nearest
// focusable element in their direction, as Android TV's own UI does. The
// WebView has no such spatial navigation of its own. Within the topmost
// `[data-focus-scope]` (an open sheet) only, so the focus cannot wander
// behind it. Text fields keep the arrows while the caret can still move.

type Direction = "up" | "down" | "left" | "right";

const KEYS: Record<string, Direction> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "textarea:not([disabled])",
  "select:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

/** Where the focus may go: the topmost focus scope, else the page. */
function scope(): ParentNode {
  const scopes = document.querySelectorAll("[data-focus-scope]");
  return scopes.length ? scopes[scopes.length - 1] : document;
}

function visible(element: HTMLElement): boolean {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 && getComputedStyle(element).visibility !== "hidden";
}

export function focusables(root: ParentNode = scope()): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(visible);
}

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Index of the best of [candidates] to go to from [from] in [direction], or
 * -1: the nearest one past [from]'s edge that way, off-axis distance counting
 * double, and one that overlaps [from] across the direction preferred.
 */
export function nearest(from: Box, candidates: Box[], direction: Direction): number {
  let best = -1;
  let bestScore = Infinity;
  const horizontal = direction === "left" || direction === "right";
  candidates.forEach((to, index) => {
    // How far past the edge, along the direction; a little overlap allowed.
    const along =
      direction === "right" ? to.left - from.right
      : direction === "left" ? from.left - to.right
      : direction === "down" ? to.top - from.bottom
      : from.top - to.bottom;
    if (along < -4) return;
    const overlap = horizontal
      ? Math.min(from.bottom, to.bottom) - Math.max(from.top, to.top)
      : Math.min(from.right, to.right) - Math.max(from.left, to.left);
    const across = overlap > 0 ? 0 : -overlap;
    const score = Math.max(along, 0) + across * 2 + (overlap > 0 ? 0 : 1000);
    if (score < bestScore) {
      bestScore = score;
      best = index;
    }
  });
  return best;
}

/** Whether a text field still has somewhere to move its caret that way. */
function fieldKeeps(element: Element, direction: Direction): boolean {
  if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) return false;
  if (element instanceof HTMLInputElement && !["text", "search", "url", "password", "number", "email", ""].includes(element.type)) {
    return false;
  }
  const start = element.selectionStart ?? 0;
  const end = element.selectionEnd ?? 0;
  const length = element.value.length;
  if (direction === "left") return start > 0;
  if (direction === "right") return end < length;
  // Up and down move between the lines of a text area only.
  if (!(element instanceof HTMLTextAreaElement)) return false;
  return direction === "up" ? element.value.lastIndexOf("\n", start - 1) >= 0 : element.value.indexOf("\n", end) >= 0;
}

/**
 * Coming into the tab rail from the page lands on the current tab, not on
 * whichever is level with the control left behind.
 */
function entering(target: HTMLElement, from: Element): HTMLElement {
  const rail = target.closest("[data-nav-rail]");
  if (!rail || rail.contains(from)) return target;
  return rail.querySelector<HTMLElement>("[aria-current=page]") ?? target;
}

function focus(element: HTMLElement) {
  element.focus({ preventScroll: true });
  element.scrollIntoView({ block: "nearest", inline: "nearest" });
}

/** Focuses the view's `[data-autofocus]` element, else its first focusable. */
export function focusFirst(root: ParentNode = scope()) {
  const preferred = root.querySelector<HTMLElement>("[data-autofocus]");
  const target = preferred && visible(preferred) ? preferred : focusables(root)[0];
  if (target) focus(target);
}

function onKey(event: KeyboardEvent) {
  const direction = KEYS[event.key];
  if (!direction || event.altKey || event.ctrlKey || event.metaKey) return;
  const current = document.activeElement;
  const root = scope();
  if (current && current !== document.body && root.contains(current) && fieldKeeps(current, direction)) return;
  const candidates = focusables(root).filter((element) => element !== current);
  if (!current || current === document.body || !root.contains(current)) {
    event.preventDefault();
    focusFirst(root);
    return;
  }
  const index = nearest(current.getBoundingClientRect(), candidates.map((element) => element.getBoundingClientRect()), direction);
  if (index >= 0) {
    event.preventDefault();
    focus(entering(candidates[index], current));
  }
}

let installed = false;

/** Turns D-pad navigation on (TV only). */
export function installSpatialNav() {
  if (installed) return;
  installed = true;
  window.addEventListener("keydown", onKey);
}
