<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";

import AppIcon from "../components/AppIcon.vue";
import ConnectionPane from "../components/ConnectionPane.vue";
import ServerAvatar from "../components/ServerAvatar.vue";
import ServerList from "../components/ServerList.vue";
import { formatDuration } from "../lib/format";
import { nameLine, parseServerName } from "../lib/serverName";
import { useAppStore } from "../stores/app";

/**
 * The connection screen. A wide window: the connection and the servers side
 * by side. A phone or a narrow window: one page, the connection first (the
 * switch in the middle while scrolled to the top), the servers under it,
 * and a small bar with the switch once it has scrolled away. A TV: the
 * connection alone; its servers are a tab of their own.
 */
const store = useAppStore();
const { t } = useI18n();
const route = useRoute();

const phone = computed(() => !store.wide && !store.isTv);
const root = ref<HTMLElement | null>(null);

/** "/servers" and links to the list land here with #servers. */
async function showList() {
  if (route.hash !== "#servers" || store.isTv) return;
  await nextTick();
  const list = root.value?.querySelector<HTMLElement>("#server-list");
  list?.scrollIntoView({ block: "start" });
  list?.querySelector<HTMLElement>("[data-server-search]")?.focus({ preventScroll: true });
}
watch(() => route.hash, showList);

// The mini bar: shown once the ring is out of sight.
const ringVisible = ref(true);
let observer: IntersectionObserver | null = null;
async function watchRing() {
  observer?.disconnect();
  observer = null;
  ringVisible.value = true;
  if (!phone.value) return;
  await nextTick();
  const ring = root.value?.querySelector("[data-ring]");
  if (!ring) return;
  observer = new IntersectionObserver(([entry]) => (ringVisible.value = entry.isIntersecting), {
    root: document.querySelector("main"),
    rootMargin: "-40px 0px 0px 0px",
  });
  observer.observe(ring);
}
watch(phone, watchRing);

const connected = computed(() => store.status === "connected");
const busy = computed(() => store.status === "connecting" || store.status === "disconnecting");
const server = computed(() => {
  const active = store.connection?.serverId;
  return store.servers.find((item) => item.id === active) ?? store.selectedServer;
});
const name = computed(() => (server.value ? parseServerName(server.value.name) : null));

const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | undefined;
const state = computed(() => {
  const since = store.connection?.since;
  const text = t(`status.${store.status}`);
  return connected.value && since ? `${text} · ${formatDuration(now.value / 1000 - since)}` : text;
});

onMounted(() => {
  ticker = setInterval(() => (now.value = Date.now()), 1000);
  void watchRing();
  void showList();
});
onBeforeUnmount(() => {
  clearInterval(ticker);
  observer?.disconnect();
});
</script>

<template>
  <div ref="root" class="connect" :class="{ wide: store.wide, tv: store.isTv }">
    <div v-if="phone" class="mini" :class="{ show: !ringVisible, on: connected }" :aria-hidden="ringVisible">
      <ServerAvatar v-if="name" :name="name" />
      <div class="min-w-0 flex-1">
        <div class="truncate font-semibold">{{ name ? nameLine(name) : t("home.noServer") }}</div>
        <div class="truncate text-xs text-muted">{{ state }}</div>
      </div>
      <button
        class="mini-power"
        :disabled="busy || !server"
        :tabindex="ringVisible ? -1 : 0"
        :aria-label="connected ? t('home.disconnect') : t('home.connect')"
        @click="store.toggleConnection()"
      >
        <AppIcon name="power" :size="20" />
      </button>
    </div>

    <ConnectionPane />
    <ServerList v-if="!store.isTv" />
  </div>
</template>

<style scoped>
.connect.wide {
  display: grid;
  grid-template-columns: minmax(380px, 460px) minmax(0, 1fr);
  min-height: 100%;
}
.connect.tv {
  height: 100%;
}

/* A phone's bar, under its status bar too: <main> is padded by the safe
   area, and the bar reaches up over that padding. */
.mini {
  --inset: env(safe-area-inset-top, 0px);
  position: sticky;
  top: calc(0px - var(--inset));
  z-index: 4;
  display: flex;
  align-items: center;
  gap: 0.625rem;
  height: calc(3.625rem + var(--inset));
  margin-bottom: calc(-3.625rem - var(--inset));
  padding: calc(0.5rem + var(--inset)) 0.875rem 0.5rem;
  border-bottom: 1px solid var(--color-line);
  background: color-mix(in srgb, var(--color-bg) 88%, transparent);
  backdrop-filter: blur(10px);
  font-size: 0.9375rem;
  opacity: 0;
  transform: translateY(-100%);
  visibility: hidden;
  transition:
    transform 0.2s var(--ease-out),
    opacity 0.2s var(--ease-out),
    visibility 0.2s;
}
.mini.show {
  opacity: 1;
  transform: none;
  visibility: visible;
}
.mini :deep(.avatar) {
  width: 1.875rem;
  height: 1.875rem;
}
.mini-power {
  display: grid;
  flex: none;
  place-items: center;
  width: 2.375rem;
  height: 2.375rem;
  border-radius: 50%;
  background: var(--color-raised);
  color: var(--color-muted);
}
.mini.on .mini-power {
  background: var(--color-accent);
  color: var(--color-accent-ink);
}
.mini-power:disabled {
  opacity: 0.6;
}
</style>
