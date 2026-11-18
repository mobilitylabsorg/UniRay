<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import ServiceCheckModal from "../components/ServiceCheckModal.vue";
import SpeedTestModal from "../components/SpeedTestModal.vue";
import { api, type Mode } from "../lib/api";
import { formatBytes, formatDuration, formatSpeed, serverMeta } from "../lib/format";
import { flagOf, withFlag } from "../lib/flag";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const showServiceCheck = ref(false);
const showSpeedTest = ref(false);

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
/**
 * Where the traffic leaves: the server a balancer group picked when that is
 * not the one shown above, else the exit address with its country's flag.
 */
const exit = computed(() => {
  const result = check.value;
  if (!result) return "";
  if (result.server && result.server !== server.value?.name) {
    return t("home.exitVia", { name: withFlag(result.server, result.country) });
  }
  if (result.ip) return `${flagOf(result.country)} ${t("home.exitIp", { ip: result.ip })}`.trim();
  return flagOf(result.country);
});
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

const corp = computed(() => (connected.value ? (store.connection?.corp ?? null) : null));
const CORP_DOT = { connecting: "bg-muted animate-pulse", up: "bg-good", down: "bg-danger" } as const;

const showMode = computed(() => store.tunSupported && store.settings !== null);

const MODES: { value: Mode; label: string }[] = [
  { value: "proxy", label: "home.modeProxy" },
  { value: "tun", label: "home.modeTun" },
];
</script>

<template>
  <!-- One column on a phone: the switch, then the details under it. Two on a
       wide window: the switch on the left, the details on the right. -->
  <div
    class="mx-auto flex min-h-full w-full max-w-5xl flex-col gap-8 px-4 py-6 md:px-8 lg:flex-row lg:items-center lg:gap-14 lg:py-10"
  >
    <section class="flex flex-1 flex-col items-center justify-center gap-5 lg:basis-0">
      <button
        class="relative flex size-36 items-center justify-center rounded-full border-2 transition-[color,background-color,border-color,box-shadow,transform,opacity] duration-300 active:scale-95 disabled:opacity-60 disabled:active:scale-100 lg:size-44 [@media(max-height:760px)]:size-32"
        :class="
          connected
            ? 'border-accent bg-accent text-accent-ink shadow-[0_8px_40px_-12px_var(--color-accent)]'
            : 'border-line bg-surface text-muted hover:border-accent hover:text-accent'
        "
        :disabled="busy || !server"
        :aria-label="connected ? t('home.disconnect') : t('home.connect')"
        data-autofocus
        @click="store.toggleConnection()"
      >
        <span v-if="connected" class="halo pointer-events-none absolute inset-[-2px] rounded-full border border-accent" />
        <Transition name="fade">
          <span v-if="busy" class="absolute inset-[-2px] animate-spin rounded-full border-2 border-transparent border-t-accent" />
        </Transition>
        <AppIcon name="power" :size="56" class="transition-transform duration-300" :class="connected ? 'scale-105' : ''" />
      </button>

      <div class="text-center" aria-live="polite">
        <Transition name="fade" mode="out-in">
          <div :key="store.status" class="text-xl font-semibold tracking-tight" :class="connected ? 'text-accent' : ''">
            {{ t(`status.${store.status}`) }}
          </div>
        </Transition>
        <div class="mt-0.5 h-5 text-sm text-muted tabular-nums">
          <Transition name="fade">
            <span v-if="uptime">{{ uptime }}</span>
          </Transition>
        </div>
      </div>

      <!-- The server: what the switch connects to, and the way to change it;
           a line of text, set apart from the status. -->
      <RouterLink
        to="/servers"
        class="group mt-3 flex max-w-full flex-col items-center rounded-xl px-3 py-1.5 text-center transition-colors"
      >
        <span class="flex max-w-full items-center gap-1 font-medium transition-colors group-hover:text-accent">
          <span class="truncate">{{ server ? server.name : t("home.noServer") }}</span>
          <AppIcon name="chevron" :size="16" class="shrink-0 text-muted transition-colors group-hover:text-accent" />
        </span>
        <span class="max-w-full truncate text-xs text-muted">{{ server ? serverMeta(server) : t("home.chooseServer") }}</span>
      </RouterLink>

      <Transition name="pop">
        <p
          v-if="error"
          class="selectable flex w-full max-w-sm items-start gap-2 rounded-xl bg-danger-soft px-3 py-2 text-sm text-danger"
          role="alert"
        >
          <AppIcon name="alert" :size="18" class="mt-0.5 shrink-0" />
          <span class="min-w-0 break-words">{{ error }}</span>
        </p>
      </Transition>
    </section>

    <section class="rise mx-auto flex w-full max-w-sm flex-col gap-3 lg:mx-0 lg:max-w-md lg:flex-1 lg:basis-0">
      <!-- The details of the connection and the mode, in one group. -->
      <div class="card divide-y divide-line overflow-hidden">
        <div class="grid grid-cols-2 divide-x divide-line transition-opacity duration-300" :class="connected ? '' : 'opacity-60'">
          <div class="px-4 py-3">
            <div class="flex items-center gap-1 text-xs text-muted">
              <AppIcon name="down" :size="14" />{{ t("home.download") }}
            </div>
            <div class="mt-0.5 text-lg font-semibold tracking-tight tabular-nums">{{ formatSpeed(store.traffic.downSpeed) }}</div>
            <div class="text-xs text-muted tabular-nums">{{ t("home.totalDown") }} {{ formatBytes(store.traffic.down) }}</div>
          </div>
          <div class="px-4 py-3">
            <div class="flex items-center gap-1 text-xs text-muted">
              <AppIcon name="up" :size="14" />{{ t("home.upload") }}
            </div>
            <div class="mt-0.5 text-lg font-semibold tracking-tight tabular-nums">{{ formatSpeed(store.traffic.upSpeed) }}</div>
            <div class="text-xs text-muted tabular-nums">{{ t("home.totalUp") }} {{ formatBytes(store.traffic.up) }}</div>
          </div>
        </div>

        <template v-if="connected">
          <div class="flex items-center gap-3 py-2.5 pr-2 pl-4">
            <Transition name="fade" mode="out-in">
              <div v-if="check" :key="check.at" class="min-w-0 flex-1">
                <div class="flex items-center gap-1.5 text-sm font-medium" :class="check.ok ? 'text-good' : 'text-danger'">
                  <AppIcon :name="check.ok ? 'check' : 'alert'" :size="16" class="shrink-0" />
                  <span class="shrink-0">
                    {{ check.ok ? t("home.checkOk") : t("home.checkFailed") }}
                    <template v-if="check.ok && check.latencyMs !== null"> · {{ t("home.latency", { ms: check.latencyMs }) }}</template>
                  </span>
                  <span class="ml-auto min-w-0 truncate pl-2 text-xs font-normal text-muted">{{ checkedAgo }}</span>
                </div>
                <div v-if="check.ok ? exit : check.error" class="truncate pl-[1.375rem] text-xs text-muted">
                  <template v-if="check.ok">{{ exit }}</template>
                  <span v-else class="selectable">{{ check.error }}</span>
                </div>
              </div>
              <div v-else class="min-w-0 flex-1 text-sm text-muted">{{ t("home.checking") }}</div>
            </Transition>
            <button
              class="icon-btn"
              :disabled="checking"
              :aria-label="t('home.check')"
              :title="t('home.check')"
              @click="runCheck"
            >
              <AppIcon name="refresh" :size="18" :class="checking ? 'animate-spin' : ''" />
            </button>
          </div>

          <RouterLink v-if="corp" to="/routing" class="row-button py-2.5 pr-3">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2 text-sm font-medium">
                <span class="size-2 shrink-0 rounded-full" :class="CORP_DOT[corp.state]" aria-hidden="true" />
                <span class="truncate">{{ t("corp.title") }}</span>
              </div>
              <div class="truncate pl-4 text-xs text-muted">
                {{ t(`corp.state.${corp.state}`) }}<template v-if="corp.address"> · {{ corp.address }}</template>
              </div>
            </div>
            <AppIcon name="chevron" :size="18" class="shrink-0 text-muted" />
          </RouterLink>
        </template>

        <div v-if="showMode && store.settings" class="flex items-center gap-3 px-4 py-3">
          <div class="min-w-0 flex-1">
            <div class="text-sm font-medium">{{ t("home.mode") }}</div>
            <div class="truncate text-xs text-muted">
              <template v-if="store.settings.mode === 'tun'">{{ t("home.tunAdminHint") }}</template>
              <template v-else>
                <span class="selectable tabular-nums">127.0.0.1:{{ store.settings.mixedPort }}</span>
                <template v-if="store.settings.proxyAuth"> · {{ t("settings.proxyAuth").toLowerCase() }}</template>
              </template>
            </div>
          </div>
          <div class="flex shrink-0 rounded-xl bg-raised p-0.5" role="radiogroup" :aria-label="t('home.mode')">
            <button
              v-for="mode in MODES"
              :key="mode.value"
              role="radio"
              :aria-checked="store.settings.mode === mode.value"
              class="h-8 rounded-[10px] px-3 text-xs font-medium whitespace-nowrap transition-[color,background-color,box-shadow] duration-200 disabled:opacity-50"
              :class="store.settings.mode === mode.value ? 'bg-surface text-ink shadow-sm' : 'text-muted hover:text-ink'"
              :disabled="busy"
              @click="store.saveSettings({ mode: mode.value })"
            >
              {{ t(mode.label) }}
            </button>
          </div>
        </div>
      </div>

      <!-- Tools: there when needed, quiet otherwise. -->
      <div class="flex flex-wrap justify-center gap-1">
        <button type="button" class="btn btn-sm btn-ghost" @click="showServiceCheck = true">
          <AppIcon name="check" :size="15" />{{ t("tools.serviceCheck") }}
        </button>
        <button type="button" class="btn btn-sm btn-ghost" @click="showSpeedTest = true">
          <AppIcon name="gauge" :size="15" />{{ t("tools.speedTest") }}
        </button>
      </div>
    </section>

    <Transition name="sheet">
      <ServiceCheckModal v-if="showServiceCheck" @close="showServiceCheck = false" />
    </Transition>
    <Transition name="sheet">
      <SpeedTestModal v-if="showSpeedTest" @close="showSpeedTest = false" />
    </Transition>
  </div>
</template>
