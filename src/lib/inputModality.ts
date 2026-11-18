// Whether the user is on the pointer or the keyboard right now, as
// `data-pointer` on <html>: styles.css hides the focus ring of a clicked
// control while it is there. Browsers do this themselves through
// :focus-visible, but not all of them (WebKitGTK keeps the ring on a clicked
// link). A TV is steered with the D-pad only and never gets the attribute.

const KEYS = new Set(["Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Enter", " ", "Home", "End", "PageUp", "PageDown"]);

let installed = false;

export function installInputModality() {
  if (installed) return;
  installed = true;
  const root = document.documentElement;
  window.addEventListener(
    "pointerdown",
    () => {
      if (!root.classList.contains("tv")) root.setAttribute("data-pointer", "");
    },
    { capture: true, passive: true },
  );
  window.addEventListener(
    "keydown",
    (event) => {
      if (KEYS.has(event.key)) root.removeAttribute("data-pointer");
    },
    { capture: true },
  );
}
