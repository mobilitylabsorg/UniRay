<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { RouterLink } from "vue-router";

import type { Mode } from "../lib/api";
import { formatDuration, formatSpeed } from "../lib/format";
import { dedupeParts, parseServerName, protocolName } from "../lib/serverName";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ConnectionCheck from "./ConnectionCheck.vue";
import ServerAvatar from "./ServerAvatar.vue";
import ServerTitle from "./ServerTitle.vue";
import ServiceCheckModal from "./ServiceCheckModal.vue";
import SpeedTestModal from "./SpeedTestModal.vue";
import TrafficChart from "./TrafficChart.vue";

/**
 * The connection: the switch with the speed around it, the server it uses,
 * the traffic of the last minute, the check, the corporate network, the mode
 * and the two tools. The left pane of the wide connection screen, or the
 * whole screen on a phone, a narrow window or a TV.
 */
const store = useAppStore();
const { t } = useI18n();
const full = computed(() => !store.wide);

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
// While connected, the server the connection actually uses.
const server = computed(() => {
  const active = store.connection?.serverId;
  return store.servers.find((item) => item.id === active) ?? store.selectedServer;
});
const name = computed(() => (server.value ? parseServerName(server.value.name) : null));
const subtitle = computed(() => {
  const current = server.value;
  if (!current || !name.value) return "";
  const kind = current.protocol === "balancer" ? t("servers.protoBalancer") : protocolName(current.protocol);
  return dedupeParts([...name.value.details, kind, current.transport, current.security]).join(" · ");
});
const error = computed(() => (store.connection?.error ? store.describe(store.connection.error) : null));

// The ring: the download speed on a log scale up to about 100 Mbit/s.
const ARC = 433.5;
const RING_MAX_KB = (100 * 1024) / 8;
const arcOffset = computed(() => {
  if (!connected.value) return ARC;
  const kb = store.traffic.downSpeed / 1024;
  const share = Math.min(1, Math.log10(1 + kb) / Math.log10(1 + RING_MAX_KB));
  return ARC * (1 - share);
});

/** The corporate network and the mode; under the switch on a phone, with
 * the check too in the wide pane. */
const mode = computed(() => store.tunSupported && store.settings !== null);
const corp = computed(() => (connected.value ? (store.connection?.corp ?? null) : null));
const CORP_DOT = { connecting: "bg-muted animate-pulse", up: "bg-accent", down: "bg-danger" } as const;

const MODES: { value: Mode; label: string }[] = [
  { value: "proxy", label: "home.modeProxyShort" },
  { value: "tun", label: "home.modeTun" },
];
</script>

<template>
  <!-- side: the left pane of a wide window. full: the whole screen on a phone,
       a narrow window or a TV, with the switch in the middle of it. -->
  <section class="pane" :class="[full ? 'full' : 'side', { on: connected }]">
    <div v-if="full" class="top">
      <template v-if="connected">
        <TrafficChart />
        <div class="section lines">
          <ConnectionCheck />
        </div>
      </template>
    </div>

    <div class="dial" :class="{ busy }" data-ring>
      <svg class="track" viewBox="0 0 200 200" aria-hidden="true">
        <circle class="text-line" cx="100" cy="100" r="92" fill="none" stroke="currentColor" stroke-width="6" stroke-linecap="round" stroke-dasharray="433.5 578" />
        <circle
          class="arc text-accent"
          cx="100"
          cy="100"
          r="92"
          fill="none"
          stroke="currentColor"
          stroke-width="6"
          stroke-linecap="round"
          stroke-dasharray="433.5 578"
          :stroke-dashoffset="busy ? 520 : arcOffset"
        />
      </svg>
      <button
        class="power"
        :disabled="busy || !server"
        :aria-label="connected ? t('home.disconnect') : t('home.connect')"
        data-autofocus
        @click="store.toggleConnection()"
      >
        <AppIcon name="power" :size="52" />
      </button>
      <div class="dial-speed tabular-nums">
        <template v-if="connected">↓ {{ formatSpeed(store.traffic.downSpeed) }}</template>
      </div>
    </div>

    <div class="bottom">
      <div class="info">
        <div class="status" aria-live="polite">
          <b>{{ t(`status.${store.status}`) }}</b>
          <span class="tabular-nums text-muted">{{ uptime ?? " " }}</span>
        </div>

        <!-- On its own screen the server is the way to the list. -->
        <component :is="full ? RouterLink : 'div'" v-bind="full ? { to: '/servers' } : {}" class="current" :title="server?.name">
          <template v-if="server && name">
            <ServerAvatar :name="name" />
            <div class="min-w-0">
              <div class="current-title">
                <ServerTitle :name="name" />
                <AppIcon v-if="full" name="chevron" :size="16" class="shrink-0 text-muted" />
              </div>
              <div class="current-sub truncate text-muted">{{ subtitle }}</div>
            </div>
          </template>
          <div v-else class="current-empty">
            <div class="font-medium">{{ t("home.noServer") }}</div>
            <div class="text-muted">{{ t("home.chooseServer") }}</div>
          </div>
        </component>

        <Transition name="pop">
          <p v-if="error" class="selectable error" role="alert">
            <AppIcon name="alert" :size="18" class="mt-0.5 shrink-0" />
            <span class="min-w-0 break-words">{{ error }}</span>
          </p>
        </Transition>
      </div>

      <div class="rest">
        <TrafficChart v-if="!full && connected" class="section" />

        <!-- The check and the corporate network while connected, a hint
             otherwise; the mode either way. -->
        <div v-if="!connected || !full || corp || mode" class="section" :class="{ lines: connected }">
          <template v-if="connected">
            <ConnectionCheck v-if="!full" />

            <RouterLink v-if="corp" to="/routing" class="sline link">
              <span class="size-2 shrink-0 rounded-full" :class="CORP_DOT[corp.state]" aria-hidden="true" />
              <div class="sline-body">
                <div class="sline-title">{{ t("corp.title") }}</div>
                <div class="sline-text">
                  {{ t(`corp.state.${corp.state}`) }}<template v-if="corp.address"> · {{ corp.address }}</template>
                </div>
              </div>
              <AppIcon name="chevron" :size="20" class="shrink-0 text-muted" />
            </RouterLink>
          </template>
          <p v-else class="hint">{{ busy ? t(`status.${store.status}`) : t(full ? "home.pickHintTab" : "home.pickHint") }}</p>

          <div v-if="mode && store.settings" class="sline">
            <div class="sline-body">
              <div class="sline-title">{{ t("home.mode") }}</div>
              <div class="sline-text">
                <template v-if="store.settings.mode === 'tun'">{{ t("home.tunAdminHint") }}</template>
                <template v-else>
                  <span class="selectable tabular-nums">127.0.0.1:{{ store.settings.mixedPort }}</span>
                  <template v-if="store.settings.proxyAuth"> · {{ t("settings.proxyAuth").toLowerCase() }}</template>
                </template>
              </div>
            </div>
            <div class="seg" role="radiogroup" :aria-label="t('home.mode')">
              <button
                v-for="mode in MODES"
                :key="mode.value"
                role="radio"
                :aria-checked="store.settings.mode === mode.value"
                :class="{ on: store.settings.mode === mode.value }"
                :disabled="busy"
                @click="store.saveSettings({ mode: mode.value })"
              >
                {{ t(mode.label) }}
              </button>
            </div>
          </div>
        </div>

        <div class="tools">
          <button type="button" @click="showServiceCheck = true">
            <AppIcon name="check" :size="15" />{{ t("tools.serviceCheck") }}
          </button>
          <button type="button" @click="showSpeedTest = true">
            <AppIcon name="gauge" :size="15" />{{ t("tools.speedTest") }}
          </button>
        </div>
      </div>
    </div>

    <Transition name="sheet">
      <ServiceCheckModal v-if="showServiceCheck" @close="showServiceCheck = false" />
    </Transition>
    <Transition name="sheet">
      <SpeedTestModal v-if="showSpeedTest" @close="showSpeedTest = false" />
    </Transition>
  </section>
</template>

<style scoped>
/* Sizes follow the approved mock, in rem (px / 16) so that a TV scales them. */
.pane {
  /* rem, not the body's 15px, so that a TV scales the text too. */
  font-size: 0.9375rem;
}

.dial {
  position: relative;
  display: grid;
  flex: none;
  place-items: center;
  width: 11.5rem;
  height: 11.5rem;
  border-radius: 50%;
}
.track {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  transform: rotate(135deg);
}
.arc {
  transition: stroke-dashoffset 0.6s ease;
}
.dial.busy .track {
  animation: dial-spin 1.1s linear infinite;
}
@keyframes dial-spin {
  to {
    transform: rotate(495deg);
  }
}
.power {
  display: grid;
  place-items: center;
  width: 7.75rem;
  height: 7.75rem;
  border: 1px solid var(--color-line);
  border-radius: 50%;
  background: var(--color-surface);
  color: var(--color-muted);
  transition:
    color 0.3s,
    background-color 0.3s,
    border-color 0.3s,
    box-shadow 0.3s,
    transform 0.15s;
}
.power:hover:not(:disabled) {
  border-color: var(--color-accent);
  color: var(--color-accent);
}
.power:active:not(:disabled) {
  transform: scale(0.96);
}
.power:disabled {
  opacity: 0.6;
}
.on .power {
  border-color: var(--color-accent);
  background: var(--color-accent);
  color: var(--color-accent-ink);
  box-shadow: 0 10px 50px -14px var(--color-accent);
}
.on .power:hover:not(:disabled) {
  color: var(--color-accent-ink);
}
.power :deep(svg) {
  width: 3.25rem;
  height: 3.25rem;
  stroke-width: 2.2;
}
.dial-speed {
  position: absolute;
  bottom: 0.125rem;
  left: 50%;
  transform: translateX(-50%);
  color: var(--color-muted);
  font-size: 0.6875rem;
  white-space: nowrap;
}

.info {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 1.125rem;
}
.status {
  text-align: center;
}
.status b {
  display: block;
  font-size: 1.4375rem;
  letter-spacing: -0.01em;
}
.on .status b {
  color: var(--color-accent);
}
.status span {
  font-size: 0.875rem;
}

.current {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 0.875rem;
  max-width: 100%;
  border-radius: 0.75rem;
  text-align: left;
}
a.current {
  padding: 0.25rem 0.5rem;
  transition: background-color 0.15s;
}
a.current:hover {
  background: var(--color-raised);
}
.current-title {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  font-size: 1.0625rem;
  font-weight: 600;
}
.current-sub {
  font-size: 0.8125rem;
}
.current-empty {
  font-size: 0.875rem;
  text-align: center;
}
.error {
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
  max-width: 24rem;
  padding: 0.5rem 0.75rem;
  border-radius: 0.75rem;
  background: var(--color-danger-soft);
  color: var(--color-danger);
  font-size: 0.875rem;
}

.section {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  padding-top: 1.5rem;
  border-top: 1px solid var(--color-line);
}
.section.lines {
  gap: 0.25rem;
}
.sline.link {
  border-radius: 0.625rem;
}
.sline.link:hover .sline-title {
  color: var(--color-accent);
}
.seg {
  display: flex;
  flex: none;
  padding: 2px;
  border-radius: 0.6875rem;
  background: var(--color-raised);
}
.seg button {
  padding: 0.4375rem 0.875rem;
  border-radius: 0.5625rem;
  color: var(--color-muted);
  font-size: 0.8125rem;
  font-weight: 500;
  transition:
    color 0.2s,
    background-color 0.2s;
}
.seg button:hover:not(:disabled) {
  color: var(--color-ink);
}
.seg button.on {
  background: var(--color-surface);
  box-shadow: 0 1px 2px rgb(0 0 0 / 0.15);
  color: var(--color-ink);
}
.seg button:disabled {
  opacity: 0.5;
}
.hint {
  margin: 0;
  color: var(--color-muted);
  font-size: 0.8125rem;
  text-align: center;
}
.tools {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 0.5rem 1.375rem;
  padding-top: 0.375rem;
  color: var(--color-muted);
  font-size: 0.8125rem;
}
.tools button {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  border-radius: 0.5rem;
}
.tools button:hover {
  color: var(--color-ink);
}

/* --- side: the left pane of a wide window, as tall as the window and staying
   put while the list scrolls. The parts follow each other as in the mock. */
.side {
  position: sticky;
  top: 0;
  display: flex;
  flex-direction: column;
  gap: 2rem;
  height: 100vh;
  overflow: auto;
  padding: 2.5rem 2.5rem 1.75rem;
  border-right: 1px solid var(--color-line);
}
.side .bottom,
.side .rest {
  display: contents;
}
.side .dial {
  width: 14rem;
  height: 14rem;
  margin: 0.75rem auto 0;
}
.side .power {
  width: 9.375rem;
  height: 9.375rem;
}
/* The switch, state and server read as one group. */
.side .info {
  margin-top: -0.875rem;
}
.side .tools {
  margin-top: auto;
}

/* A laptop's height: the whole pane fits without scrolling. */
@media (max-height: 880px) {
  .side {
    gap: 1.375rem;
    padding-top: 1.75rem;
  }
  .side .dial {
    width: 11.75rem;
    height: 11.75rem;
    margin-top: 0;
  }
  .side .power {
    width: 8rem;
    height: 8rem;
  }
  .side .info {
    gap: 0.75rem;
    margin-top: -0.625rem;
  }
  .side .section {
    --chart-gap: 0.75rem;
    --chart-height: 4rem;
    gap: 0.75rem;
    padding-top: 1.125rem;
  }
  .side .section.lines {
    gap: 0.25rem;
  }
  .side {
    --sline-height: 2.625rem;
  }
}

/* --- full: the screen of a phone, a narrow window or a TV. The switch sits in
   the middle: the traffic above it, everything else under it, each half as
   tall as the other while both fit. Only a very short window scrolls. */
.full {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  grid-template-rows: minmax(max-content, 1fr) auto minmax(max-content, 1fr);
  row-gap: 1.25rem;
  height: 100%;
  padding: 1rem 1.25rem;
}
.full > * {
  width: 100%;
  max-width: 32rem;
  margin-inline: auto;
}
.full .dial {
  width: 11.5rem;
}
.full .top {
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: 0.5rem;
}
.full .bottom,
.full .rest {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}
.full .rest {
  flex: 1;
}
.full .info {
  gap: 0.75rem;
}
.full .section {
  padding-top: 1rem;
}
.full .tools {
  margin-top: auto;
}

/* A short screen (the default desktop window, small phones, a TV): smaller
   switch, chart and lines rather than a page that scrolls. */
@media (max-height: 780px) {
  .full {
    --chart-gap: 0.5rem;
    --chart-height: 3.25rem;
    row-gap: 0.875rem;
    padding-block: 0.75rem;
  }
  .full .dial {
    width: 9.75rem;
    height: 9.75rem;
  }
  .full .power {
    width: 6.75rem;
    height: 6.75rem;
  }
  .full .power :deep(svg) {
    width: 2.75rem;
    height: 2.75rem;
  }
  .full .info {
    gap: 0.5rem;
  }
  .full .status b {
    font-size: 1.25rem;
  }
  .full .bottom,
  .full .rest {
    gap: 0.625rem;
  }
  .full .section {
    padding-top: 0.625rem;
  }
  .full {
    --sline-height: 2.5rem;
  }
}

/* A small phone: the chart's legend goes, the rest gets smaller still. */
@media (max-height: 660px) {
  .full {
    --chart-height: 2.5rem;
    --sline-height: 2.25rem;
    row-gap: 0.625rem;
  }
  .full :deep(.legend) {
    display: none;
  }
  .full .dial {
    width: 8.5rem;
    height: 8.5rem;
  }
  .full .power {
    width: 6rem;
    height: 6rem;
  }
  .full .power :deep(svg) {
    width: 2.5rem;
    height: 2.5rem;
  }
  .full .status b {
    font-size: 1.125rem;
  }
  .full .info {
    gap: 0.375rem;
  }
  .full .tools {
    padding-top: 0;
  }
}

/* Landscape and short: a TV, a phone on its side, a wide but low window. The
   switch and its state on the left, in the middle of the height; the
   traffic and the rest on the right. */
@media (orientation: landscape) and (max-height: 600px) {
  .full {
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.25fr);
    grid-template-rows: minmax(0, 1fr) auto minmax(0, 1fr);
    column-gap: 2rem;
  }
  .full > * {
    max-width: none;
  }
  .full .bottom {
    display: contents;
  }
  .full .dial {
    grid-row: 2;
    grid-column: 1;
  }
  .full .info {
    grid-row: 3;
    grid-column: 1;
    align-self: start;
  }
  .full .top {
    grid-row: 1 / 4;
    grid-column: 2;
    align-self: start;
  }
  .full .rest {
    grid-row: 1 / 4;
    grid-column: 2;
    flex: none;
    align-self: end;
  }
  /* Nothing above: the rest in the middle of the right half. */
  .full .top:empty ~ .bottom .rest {
    align-self: center;
  }
}
</style>
