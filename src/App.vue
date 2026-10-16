<script setup lang="ts">
import { nextTick, onMounted, watch } from "vue";

import BottomNav from "./components/BottomNav.vue";
import ToastStack from "./components/ToastStack.vue";
import { api, toCmdError } from "./lib/api";
import { installBack } from "./lib/backStack";
import { focusFirst, installSpatialNav } from "./lib/spatialNav";
import { followSystemColors } from "./lib/theme";
import { router } from "./router";
import { useAppStore } from "./stores/app";

const store = useAppStore();

installBack(router);

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

function focusPage() {
  if (!store.isTv) return;
  const main = document.querySelector("main");
  if (main) focusFirst(main);
}

onMounted(async () => {
  // Desktop has no system palette; the call resolves to null there.
  followSystemColors();
  (window as unknown as { __unirayHandleUri?: (uri: string) => void }).__unirayHandleUri = (uri: string) => {
    if (uri.startsWith("uniray://connect")) {
      if (store.status === "disconnected") {
        void store.toggleConnection();
      }
    } else if (uri.startsWith("uniray://disconnect")) {
      if (store.status === "connected") {
        void store.toggleConnection();
      }
    } else if (uri.startsWith("uniray://toggle")) {
      void store.toggleConnection();
    } else if (uri.startsWith("uniray://import-sub")) {
      try {
        const u = new URL(uri);
        const subUrl = u.searchParams.get("url");
        if (subUrl) {
          void store.run(() => api.addSubscription(subUrl));
        }
      } catch {
        // ignore
      }
    }
  };

  try {
    await store.init();
    if (store.settings?.autoConnect && store.selectedServer && store.status === "disconnected") {
      void store.toggleConnection();
    }
  } catch (err) {
    store.toast(store.describe(toCmdError(err)), "error");
  }
});
</script>

<template>
  <!-- On a TV the tabs are a rail on the left, the content wider; the margins
       keep everything inside a TV's overscan. -->
  <div
    class="mx-auto flex h-full"
    :class="store.isTv ? 'max-w-6xl gap-6 px-[4vw] py-[4vh]' : 'max-w-2xl flex-col'"
  >
    <BottomNav v-if="store.isTv" vertical />
    <main class="min-h-0 flex-1 overflow-y-auto pt-[env(safe-area-inset-top)]">
      <RouterView v-if="store.snapshot" v-slot="{ Component }">
        <Transition name="page" mode="out-in" @after-enter="focusPage">
          <component :is="Component" />
        </Transition>
      </RouterView>
    </main>
    <BottomNav v-if="!store.isTv" />
    <ToastStack />
  </div>
</template>
