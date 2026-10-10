// What the Android Back key closes first: the topmost open sheet, then the
// current page (back to the previous one, or home). MainActivity asks
// `window.__unirayBack()` before Android's own handling; `false` means there
// was nothing to close here, and the app goes to the background.
import type { Router } from "vue-router";

const closers: (() => void)[] = [];

/** Registers an open sheet's close; returns the unregister. */
export function onBack(close: () => void): () => void {
  closers.push(close);
  return () => {
    const index = closers.lastIndexOf(close);
    if (index >= 0) closers.splice(index, 1);
  };
}

export function installBack(router: Router) {
  (window as unknown as { __unirayBack: () => boolean }).__unirayBack = () => {
    const close = closers[closers.length - 1];
    if (close) {
      close();
      return true;
    }
    if (router.currentRoute.value.path !== "/") {
      if (window.history.state?.back) router.back();
      else void router.replace("/");
      return true;
    }
    return false;
  };
}
