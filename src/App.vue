<script setup lang="ts">
import { nextTick, onMounted, watch } from "vue";

import BottomNav from "./components/BottomNav.vue";
import ToastStack from "./components/ToastStack.vue";
import { events, toCmdError } from "./lib/api";
import { installBack } from "./lib/backStack";
import { installInputModality } from "./lib/inputModality";
import { focusFirst, installSpatialNav } from "./lib/spatialNav";
import { followSystemColors } from "./lib/theme";
import { router } from "./router";
import { useAppStore } from "./stores/app";

const store = useAppStore();

installBack(router);
installInputModality();

// On a TV: the 10-foot layout (the `tv` class on <html>, see styles.css) and
// D-pad navigation, with the focus on each page's main control.
watch(
  () => store.isTv,
  (tv) => {
    document.documentElement.classList.toggle("tv", tv);
    if (tv) {
      installSpatialNav();
      void nextTick(() => focusPage());
    }
  },
  { immediate: true },
);

// A subscription offered by a uniray:// link is confirmed in the server
// list's import dialog (the servers tab, or the list on a wide window).
watch(
  () => store.pendingImport,
  (url) => {
    if (url) void router.push({ name: "servers" });
  },
);

// The servers are a tab of their own on a TV and part of the connection
// screen elsewhere (known once the state has arrived).
watch(
  [() => store.isTv, () => router.currentRoute.value.fullPath],
  ([tv]) => {
    const route = router.currentRoute.value;
    if (!tv && route.name === "servers") void router.replace({ name: "connect", hash: "#servers" });
    else if (tv && route.name === "connect" && route.hash === "#servers") void router.replace({ name: "servers" });
  },
);

function focusPage() {
  if (!store.isTv) return;
  const main = document.querySelector("main");
  if (main) focusFirst(main);
}

onMounted(async () => {
  // Desktop has no system palette; the call resolves to null there.
  followSystemColors();
  try {
    await store.init();
    // uniray:// links: the one the app was started with, then each as it
    // arrives (src-tauri/src/deeplink.rs). A link that says what to do wins
    // over auto-connect.
    await events.deepLink(() => void store.takeDeepLinks());
    const linked = await store.takeDeepLinks();
    if (!linked && store.settings?.autoConnect && store.selectedServer && store.status === "disconnected") {
      void store.toggleConnection();
    }
  } catch (err) {
    store.toast(store.describe(toCmdError(err)), "error");
  }
});
</script>

<template>
  <!-- A phone or a narrow window: the tabs at the bottom. A wide desktop
       window: a sidebar on the left, the page centered in the rest. A TV:
       the tabs are a rail on the left, the margins keep everything inside
       its overscan. -->
  <div
    class="flex h-full"
    :class="store.isTv ? 'mx-auto max-w-6xl gap-6 px-[4vw] py-[4vh]' : 'flex-col nav:flex-row'"
  >
    <BottomNav v-if="store.isTv" variant="rail" />
    <BottomNav v-else variant="sidebar" class="max-nav:hidden" />
    <main class="min-h-0 flex-1 overflow-y-auto pt-[env(safe-area-inset-top)] [container-type:size]">
      <RouterView v-if="store.snapshot" v-slot="{ Component }">
        <Transition name="page" mode="out-in" @after-enter="focusPage">
          <component :is="Component" />
        </Transition>
      </RouterView>
    </main>
    <BottomNav v-if="!store.isTv" class="nav:hidden" />
    <ToastStack />
  </div>
</template>
