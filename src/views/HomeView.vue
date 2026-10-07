<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import { api, type Mode } from "../lib/api";
import { formatBytes, formatDuration, formatSpeed } from "../lib/format";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  ticker = setInterval(() => (now.value = Date.now()), 1000);
});
onBeforeUnmount(() => clearInterval(ticker));

const busy = computed(() => store.status === "connecting" || store.status === "disconnecting");
const connected = computed(() => store.status === "connected");
const uptime = computed(() => {
  const since = store.connection?.since;
  return connected.value && since ? formatDuration(now.value / 1000 - since) : null;
});
// While connected, show the server the connection actually uses.
const server = computed(() => {
  const active = store.connection?.serverId;
  return store.servers.find((item) => item.id === active) ?? store.selectedServer;
});
const error = computed(() => (store.connection?.error ? store.describe(store.connection.error) : null));

const checking = ref(false);
const check = computed(() => store.connection?.check ?? null);
const checkedAgo = computed(() => {
  if (!check.value) return "";
  const minutes = Math.floor((now.value / 1000 - check.value.at) / 60);
  return t("home.checkedAgo", { ago: minutes < 1 ? t("home.justNow") : t("home.minutesAgo", { n: minutes }) });
});

async function runCheck() {
  checking.value = true;
  // The result arrives with the state as well; a failure is a toast.
  await store.run(() => api.checkConnection());
  checking.value = false;
}

const MODES: { value: Mode; label: string }[] = [
  { value: "proxy", label: "home.modeProxy" },
  { value: "tun", label: "home.modeTun" },
];
</script>

<template>
  <div class="flex min-h-full flex-col items-center gap-6 px-5 py-8">
    <div class="flex flex-1 flex-col items-center justify-center gap-6">
      <button
        class="relative flex size-44 items-center justify-center rounded-full border-4 transition-all duration-300 disabled:opacity-60"
        :class="
          connected
            ? 'border-accent bg-accent text-accent-ink shadow-[0_0_60px_-10px_var(--color-accent)]'
            : 'border-line bg-surface text-muted hover:border-accent hover:text-accent'
        "
        :disabled="busy || !server"
        :aria-label="connected ? t('home.disconnect') : t('home.connect')"
        @click="store.toggleConnection()"
      >
        <span v-if="busy" class="absolute inset-[-4px] animate-spin rounded-full border-4 border-transparent border-t-accent" />
        <AppIcon name="power" :size="64" />
      </button>

      <div class="text-center">
        <div class="text-xl font-semibold" :class="connected ? 'text-accent' : ''">
          {{ t(`status.${store.status}`) }}
        </div>
        <div class="mt-1 h-5 font-mono text-sm text-muted tabular-nums">{{ uptime }}</div>
      </div>

      <p
        v-if="error"
        class="selectable flex max-w-sm items-start gap-2 rounded-xl bg-danger-soft px-3 py-2 text-sm text-danger"
        role="alert"
      >
        <AppIcon name="alert" :size="18" class="mt-0.5 shrink-0" />
        <span class="min-w-0 break-words">{{ error }}</span>
      </p>
    </div>

    <div class="flex w-full max-w-sm flex-col gap-3">
      <RouterLink to="/servers" class="card flex items-center gap-3 px-4 py-3 transition-colors hover:border-accent">
        <AppIcon name="server" class="shrink-0 text-muted" />
        <div v-if="server" class="min-w-0 flex-1">
          <div class="truncate font-medium">{{ server.name }}</div>
          <div class="truncate text-xs text-muted">
            {{ server.protocol }} · {{ server.transport }} · {{ server.security }}
          </div>
        </div>
        <div v-else class="min-w-0 flex-1">
          <div class="font-medium">{{ t("home.noServer") }}</div>
          <div class="text-xs text-muted">{{ t("home.chooseServer") }}</div>
        </div>
        <AppIcon name="chevron" class="shrink-0 text-muted" />
      </RouterLink>

      <div class="grid grid-cols-2 gap-3">
        <div class="card px-4 py-3">
          <div class="flex items-center gap-1.5 text-xs text-muted">
            <AppIcon name="down" :size="14" />{{ t("home.download") }}
          </div>
          <div class="mt-1 font-mono text-lg tabular-nums">{{ formatSpeed(store.traffic.downSpeed) }}</div>
          <div class="text-xs text-muted">{{ t("home.totalDown") }} {{ formatBytes(store.traffic.down) }}</div>
        </div>
        <div class="card px-4 py-3">
          <div class="flex items-center gap-1.5 text-xs text-muted">
            <AppIcon name="up" :size="14" />{{ t("home.upload") }}
          </div>
          <div class="mt-1 font-mono text-lg tabular-nums">{{ formatSpeed(store.traffic.upSpeed) }}</div>
          <div class="text-xs text-muted">{{ t("home.totalUp") }} {{ formatBytes(store.traffic.up) }}</div>
        </div>
      </div>

      <div v-if="connected" class="card px-4 py-3">
        <template v-if="check">
          <div class="flex items-baseline gap-2">
            <span class="flex items-center gap-1.5 font-medium" :class="check.ok ? 'text-good' : 'text-danger'">
              <AppIcon :name="check.ok ? 'check' : 'alert'" :size="16" class="self-center" />
              {{ check.ok ? t("home.checkOk") : t("home.checkFailed") }}
            </span>
            <span v-if="check.ok && check.latencyMs !== null" class="font-mono text-sm tabular-nums" :class="check.ok ? 'text-good' : ''">
              {{ t("home.latency", { ms: check.latencyMs }) }}
            </span>
            <span class="ml-auto shrink-0 text-xs text-muted">{{ checkedAgo }}</span>
          </div>
          <div class="selectable mt-0.5 truncate font-mono text-xs text-muted">
            <template v-if="check.ok">{{ [check.country, check.ip].filter(Boolean).join(" · ") }}</template>
            <template v-else>{{ check.error }}</template>
          </div>
        </template>
        <div v-else class="text-sm text-muted">{{ t("home.checking") }}</div>
        <button class="btn btn-quiet mt-3 w-full" :disabled="checking" @click="runCheck">
          <AppIcon name="refresh" :size="16" :class="checking ? 'animate-spin' : ''" />
          {{ checking ? t("home.checking") : t("home.check") }}
        </button>
      </div>

      <template v-if="store.isDesktop && store.settings">
        <div class="card flex p-1" role="radiogroup" :aria-label="t('home.mode')">
          <button
            v-for="mode in MODES"
            :key="mode.value"
            role="radio"
            :aria-checked="store.settings.mode === mode.value"
            class="h-9 flex-1 rounded-xl text-sm font-medium transition-colors disabled:opacity-50"
            :class="store.settings.mode === mode.value ? 'bg-accent-soft text-accent' : 'text-muted hover:text-ink'"
            :disabled="busy"
            @click="store.saveSettings({ mode: mode.value })"
          >
            {{ t(mode.label) }}
          </button>
        </div>
        <p class="px-1 text-center text-xs text-muted">
          <template v-if="store.settings.mode === 'tun'">{{ t("home.tunPasswordHint") }}</template>
          <template v-else>
            {{ t("home.localProxy") }}:
            <span class="selectable font-mono">127.0.0.1:{{ store.settings.mixedPort }}</span>
            <template v-if="store.settings.proxyAuth"> · {{ t("settings.proxyAuth").toLowerCase() }}</template>
          </template>
        </p>
      </template>
    </div>
  </div>
</template>
