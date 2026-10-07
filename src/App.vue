<script setup lang="ts">
import { onMounted } from "vue";

import BottomNav from "./components/BottomNav.vue";
import ToastStack from "./components/ToastStack.vue";
import { toCmdError } from "./lib/api";
import { followSystemColors } from "./lib/theme";
import { useAppStore } from "./stores/app";

const store = useAppStore();

onMounted(async () => {
  // Desktop has no system palette; the call resolves to null there.
  followSystemColors();
  try {
    await store.init();
  } catch (err) {
    store.toast(store.describe(toCmdError(err)), "error");
  }
});
</script>

<template>
  <div class="mx-auto flex h-full max-w-2xl flex-col">
    <main class="min-h-0 flex-1 overflow-y-auto pt-[env(safe-area-inset-top)]">
      <RouterView v-if="store.snapshot" />
    </main>
    <BottomNav />
    <ToastStack />
  </div>
</template>
