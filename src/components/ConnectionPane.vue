<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { api, type Mode } from "../lib/api";
import { flagOf } from "../lib/flag";
import { formatBytes, formatDuration, formatSpeed } from "../lib/format";
import { dedupeParts, nameLine, parseServerName, protocolName } from "../lib/serverName";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ServerAvatar from "./ServerAvatar.vue";
import ServerTitle from "./ServerTitle.vue";
import ServiceCheckModal from "./ServiceCheckModal.vue";
import SpeedTestModal from "./SpeedTestModal.vue";

/**
 * The connection: the switch with the speed around it, the server it uses,
 * the traffic of the last minute, the check, the corporate network, the mode
 * and the two tools. The left pane of the connection screen, or its top.
 */
const store = useAppStore();
const { t, locale } = useI18n();

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

// The chart: the last minute, download as an area, upload as a dashed line.
const CHART_W = 340;
const CHART_H = 72;
const CHART_POINTS = 60;
const chart = computed(() => {
  const { down, up } = store.history;
  const max = Math.max(1, ...down, ...up) * 1.15;
  const points = (values: number[]) => {
    const offset = CHART_POINTS - values.length;
    return values.map((value, index) => [((index + offset) / (CHART_POINTS - 1)) * CHART_W, CHART_H - (value / max) * (CHART_H - 4) - 2]);
  };
  const path = (list: number[][]) => (list.length ? "M" + list.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join("L") : "");
  const downPoints = points(down);
  const area = downPoints.length
    ? `${path(downPoints)}L${downPoints[downPoints.length - 1][0].toFixed(1)},${CHART_H}L${downPoints[0][0].toFixed(1)},${CHART_H}Z`
    : "";
  return { down: path(downPoints), up: path(points(up)), area };
});

const checking = ref(false);
const check = computed(() => store.connection?.check ?? null);
const regionNames = computed(() => {
  try {
    return new Intl.DisplayNames([locale.value], { type: "region" });
  } catch {
    return null;
  }
});
/** Where the traffic leaves: a balancer's pick, else the exit address and country. */
const exit = computed(() => {
  const result = check.value;
  if (!result?.ok) return "";
  if (result.server && result.server !== server.value?.name) {
    const picked = parseServerName(result.server);
    const flag = flagOf(picked.country ?? result.country);
    return t("home.exitVia", { name: `${flag} ${nameLine(picked)}`.trim() });
  }
  const parts: string[] = [];
  if (result.ip) parts.push(t("home.exitIp", { ip: result.ip }));
  if (result.country) {
    const country = regionNames.value?.of(result.country.toUpperCase()) ?? result.country;
    parts.push(`${flagOf(result.country)} ${country}`.trim());
  }
  return parts.join(" · ");
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
const CORP_DOT = { connecting: "bg-muted animate-pulse", up: "bg-accent", down: "bg-danger" } as const;

const MODES: { value: Mode; label: string }[] = [
  { value: "proxy", label: "home.modeProxyShort" },
  { value: "tun", label: "home.modeTun" },
];
</script>

<template>
  <section class="pane" :class="{ on: connected, 'is-tv': store.isTv }">
    <div class="hero">
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

      <div class="status" aria-live="polite">
        <b>{{ t(`status.${store.status}`) }}</b>
        <span class="tabular-nums text-muted">{{ uptime ?? " " }}</span>
      </div>

      <div v-if="server && name" class="current" :title="server.name">
        <ServerAvatar :name="name" />
        <div class="min-w-0">
          <div class="current-title"><ServerTitle :name="name" /></div>
          <div class="current-sub truncate text-muted">{{ subtitle }}</div>
        </div>
      </div>
      <div v-else class="current-empty">
        <div class="font-medium">{{ t("home.noServer") }}</div>
        <div class="text-muted">{{ t("home.chooseServer") }}</div>
      </div>

      <Transition name="pop">
        <p v-if="error" class="selectable error" role="alert">
          <AppIcon name="alert" :size="18" class="mt-0.5 shrink-0" />
          <span class="min-w-0 break-words">{{ error }}</span>
        </p>
      </Transition>
    </div>

    <template v-if="connected">
      <div class="section">
        <div class="chart-head">
          <div class="min-w-0">
            <div class="big tabular-nums">{{ formatSpeed(store.traffic.downSpeed) }}</div>
            <div class="small text-muted">{{ t("home.chartDown", { total: formatBytes(store.traffic.down) }) }}</div>
          </div>
          <div class="min-w-0 text-right">
            <div class="font-semibold tabular-nums">{{ formatSpeed(store.traffic.upSpeed) }}</div>
            <div class="small text-muted">{{ t("home.chartUp", { total: formatBytes(store.traffic.up) }) }}</div>
          </div>
        </div>
        <div>
          <svg class="chart" :viewBox="`0 0 ${CHART_W} ${CHART_H}`" preserveAspectRatio="none" aria-hidden="true">
            <defs>
              <linearGradient id="chart-down" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stop-color="var(--color-accent)" stop-opacity="0.35" />
                <stop offset="1" stop-color="var(--color-accent)" stop-opacity="0" />
              </linearGradient>
            </defs>
            <path :d="chart.area" fill="url(#chart-down)" />
            <path :d="chart.down" fill="none" stroke="var(--color-accent)" stroke-width="2" vector-effect="non-scaling-stroke" />
            <path
              :d="chart.up"
              fill="none"
              stroke="var(--color-up)"
              stroke-width="1.5"
              stroke-dasharray="3 3"
              vector-effect="non-scaling-stroke"
            />
          </svg>
          <div class="legend text-muted">
            <span><i class="bg-accent" />{{ t("home.legendDown") }}</span>
            <span><i class="bg-up" />{{ t("home.legendUp") }}</span>
            <span class="ml-auto">{{ t("home.lastMinute") }}</span>
          </div>
        </div>
      </div>
    </template>

    <!-- The check and the corporate network while connected, a hint
         otherwise; the mode either way. -->
    <div class="section" :class="{ lines: connected }">
      <template v-if="connected">
        <div class="line">
          <div v-if="check" class="grow">
            <div class="l1" :class="check.ok ? 'text-accent' : 'text-danger'">
              <AppIcon :name="check.ok ? 'check' : 'alert'" :size="16" class="shrink-0" />
              <span class="truncate">
                {{ check.ok ? t("home.checkOk") : t("home.checkFailed") }}
                <template v-if="check.ok && check.latencyMs !== null"> · {{ t("home.latency", { ms: check.latencyMs }) }}</template>
              </span>
            </div>
            <div class="l2">
              <span v-if="!check.ok" class="selectable">{{ check.error }} · </span>
              <template v-else-if="exit">{{ exit }} · </template>
              {{ checkedAgo }}
            </div>
          </div>
          <div v-else class="grow text-muted">{{ t("home.checking") }}</div>
          <button class="icon-btn" :disabled="checking" :aria-label="t('home.check')" :title="t('home.check')" @click="runCheck">
            <AppIcon name="refresh" :size="20" :class="checking ? 'animate-spin' : ''" />
          </button>
        </div>

        <RouterLink v-if="corp" to="/routing" class="line link">
          <span class="size-2 shrink-0 rounded-full" :class="CORP_DOT[corp.state]" aria-hidden="true" />
          <div class="grow">
            <div class="l1">{{ t("corp.title") }}</div>
            <div class="l2">
              {{ t(`corp.state.${corp.state}`) }}<template v-if="corp.address"> · {{ corp.address }}</template>
            </div>
          </div>
          <AppIcon name="chevron" :size="20" class="shrink-0 text-muted" />
        </RouterLink>
      </template>
      <p v-else class="hint">{{ busy ? t(`status.${store.status}`) : t("home.pickHint") }}</p>

      <div v-if="store.tunSupported && store.settings" class="line">
        <div class="grow">
          <div class="l1">{{ t("home.mode") }}</div>
          <div class="l2">
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

    <Transition name="sheet">
      <ServiceCheckModal v-if="showServiceCheck" @close="showServiceCheck = false" />
    </Transition>
    <Transition name="sheet">
      <SpeedTestModal v-if="showSpeedTest" @close="showSpeedTest = false" />
    </Transition>
  </section>
</template>

<style scoped>
/* Sizes follow the approved mock (px / 16 = rem, so a TV scales them). The
   narrow layout is the default; the wide one is in the media rules below. */
.pane {
  /* rem, not the body's 15px, so that a TV scales the text too. */
  font-size: 0.9375rem;
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
  padding: 1.75rem 1.25rem 1.5rem;
  border-bottom: 1px solid var(--color-line);
}
.pane > * {
  width: 100%;
  max-width: 32rem;
  margin-inline: auto;
}

.hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 1.125rem;
  padding-top: 0.75rem;
}
.dial {
  position: relative;
  display: grid;
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
  text-align: left;
}
.current-title {
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
.chart-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 1rem;
}
.big {
  font-size: 1.5rem;
  font-weight: 650;
  letter-spacing: -0.02em;
}
.small {
  font-size: 0.75rem;
}
.chart {
  display: block;
  width: 100%;
  height: 5.25rem;
}
.legend {
  display: flex;
  gap: 1rem;
  font-size: 0.75rem;
}
.legend i {
  display: inline-block;
  width: 0.5rem;
  height: 0.5rem;
  margin-right: 0.3125rem;
  border-radius: 50%;
  vertical-align: 1px;
}
.line {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  min-height: 3rem;
}
.line.link {
  border-radius: 0.625rem;
}
.line.link:hover .l1 {
  color: var(--color-accent);
}
.grow {
  flex: 1;
  min-width: 0;
}
.l1 {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  font-weight: 500;
}
.l2 {
  overflow: hidden;
  color: var(--color-muted);
  font-size: 0.8125rem;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.icon-btn {
  width: 2.25rem;
  height: 2.25rem;
  border-radius: 0.625rem;
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

/* Wide: the left pane, as tall as the window and staying put while the list
   scrolls. A TV keeps the one-column layout for its D-pad. */
@media (min-width: 900px) {
  .pane:not(.is-tv) {
    position: sticky;
    top: 0;
    height: 100vh;
    overflow: auto;
    gap: 2rem;
    padding: 2.5rem 2.5rem 1.75rem;
    border-right: 1px solid var(--color-line);
    border-bottom: 0;
  }
  .pane:not(.is-tv) > * {
    max-width: none;
  }
  .pane:not(.is-tv) .dial {
    width: 14rem;
    height: 14rem;
  }
  .pane:not(.is-tv) .power {
    width: 9.375rem;
    height: 9.375rem;
  }
  .pane:not(.is-tv) .tools {
    margin-top: auto;
  }
}

/* A laptop's height: the whole pane fits without scrolling. */
@media (min-width: 900px) and (max-height: 880px) {
  .pane:not(.is-tv) {
    gap: 1.375rem;
    padding-top: 1.75rem;
  }
  .pane:not(.is-tv) .dial {
    width: 11.75rem;
    height: 11.75rem;
  }
  .pane:not(.is-tv) .power {
    width: 8rem;
    height: 8rem;
  }
  .pane:not(.is-tv) .hero {
    gap: 0.75rem;
    padding-top: 0;
  }
  .pane:not(.is-tv) .section {
    gap: 0.75rem;
    padding-top: 1.125rem;
  }
  .pane:not(.is-tv) .section.lines {
    gap: 0.25rem;
  }
  .pane:not(.is-tv) .chart {
    height: 4rem;
  }
  .pane:not(.is-tv) .line {
    min-height: 2.625rem;
  }
}
</style>
