<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";

import ConnectionPane from "../components/ConnectionPane.vue";
import ServerList from "../components/ServerList.vue";
import { useAppStore } from "../stores/app";

/**
 * The connection screen. A wide window has the connection and the servers
 * side by side; a phone, a narrow window or a TV the connection alone, with
 * the switch in the middle (the servers are a tab of their own there).
 */
const store = useAppStore();
const route = useRoute();
const root = ref<HTMLElement | null>(null);

/** "/servers" and links to the list land here with #servers on a wide window. */
async function showList() {
  if (route.hash !== "#servers" || !store.wide) return;
  await nextTick();
  const list = root.value?.querySelector<HTMLElement>("#server-list");
  list?.scrollIntoView({ block: "start" });
  list?.querySelector<HTMLElement>("[data-server-search]")?.focus({ preventScroll: true });
}
watch(() => route.hash, showList);
onMounted(showList);
</script>

<template>
  <div ref="root" class="connect" :class="{ wide: store.wide }">
    <ConnectionPane />
    <ServerList v-if="store.wide" />
  </div>
</template>

<style scoped>
.connect {
  height: 100%;
}
.connect.wide {
  display: grid;
  grid-template-columns: minmax(380px, 460px) minmax(0, 1fr);
  height: auto;
  min-height: 100%;
}
</style>
