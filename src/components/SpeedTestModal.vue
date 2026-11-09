<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";
import { api } from "../lib/api";
import {
  BUILT_IN_TARGETS,
  discoverOoklaServers,
  formatClock,
  formatRate,
  getServerDownloadUrl,
  getServerLatencyUrl,
  getServerUploadUrl,
  loadHistory,
  loadPinnedServerId,
  loadSavedScope,
  saveHistory,
  savePinnedServerId,
  saveScope,
  type SpeedTestHistoryEntry,
  type SpeedTestRunResult,
  type SpeedTestServer,
} from "../lib/speedtest";

const emit = defineEmits<{ close: [] }>();
const { t } = useI18n();

type Scope = "tunnel" | "direct" | "both";
type Phase = "idle" | "searching" | "latency" | "download" | "upload" | "done";

// State
const scope = ref<Scope>(loadSavedScope());
const servers = ref<SpeedTestServer[]>([...BUILT_IN_TARGETS]);
const selectedServerId = ref<string>(loadPinnedServerId() || "auto");
const activeServer = ref<SpeedTestServer | null>(null);
const isRunning = ref(false);
const currentPhase = ref<Phase>("idle");
const hadTunnel = ref(false);
const tunnelUp = ref(false);
const underlyingType = ref<string>("Wi-Fi");

// Results
const primaryResult = ref<SpeedTestRunResult | null>(null);
const directResult = ref<SpeedTestRunResult | null>(null);
const history = ref<SpeedTestHistoryEntry[]>(loadHistory());

// Live readout for active run
const liveMbps = ref(0.0);
const animatedMbps = ref(0.0);
const currentPing = ref<number | null>(null);
const currentJitter = ref<number | null>(null);
const currentDown = ref<number | null>(null);
const currentUp = ref<number | null>(null);

let abort = false;
let unlisten: UnlistenFn | null = null;
let animFrameId: number | null = null;

// Dynamic values displayed directly in the cards
const displayDownload = computed(() => {
  if (isRunning.value) {
    if (currentPhase.value === "download") {
      return formatRate(animatedMbps.value).value;
    }
    if (currentDown.value !== null) {
      return formatRate(currentDown.value).value;
    }
    return "—";
  }
  if (primaryResult.value?.downloadMbps !== null && primaryResult.value?.downloadMbps !== undefined) {
    return formatRate(primaryResult.value.downloadMbps).value;
  }
  return "—";
});

const displayUpload = computed(() => {
  if (isRunning.value) {
    if (currentPhase.value === "upload") {
      return formatRate(animatedMbps.value).value;
    }
    if (currentUp.value !== null) {
      return formatRate(currentUp.value).value;
    }
    return "—";
  }
  if (primaryResult.value?.uploadMbps !== null && primaryResult.value?.uploadMbps !== undefined) {
    return formatRate(primaryResult.value.uploadMbps).value;
  }
  return "—";
});

const displayPing = computed(() => {
  if (isRunning.value) {
    if (currentPing.value !== null) {
      return String(currentPing.value);
    }
    if (currentPhase.value === "latency") {
      return "…";
    }
    return "—";
  }
  if (primaryResult.value?.pingMs !== null && primaryResult.value?.pingMs !== undefined) {
    return String(primaryResult.value.pingMs);
  }
  return "—";
});

const displayJitter = computed(() => {
  const j = isRunning.value ? currentJitter.value : primaryResult.value?.jitterMs;
  if (j !== null && j !== undefined) {
    return `±${j} мс`;
  }
  return "мс";
});

// Comparison delta
const downloadDeltaPercent = computed(() => {
  const tunnel = primaryResult.value?.downloadMbps;
  const direct = directResult.value?.downloadMbps;
  if (tunnel === null || tunnel === undefined || direct === null || direct === undefined || direct <= 0) {
    return null;
  }
  return Math.round(((tunnel - direct) / direct) * 100);
});

// Phase status
const phaseText = computed(() => {
  if (!isRunning.value) {
    if (primaryResult.value) return t("tools.speedTestDone");
    return t("tools.speedTestReady");
  }
  if (currentPhase.value === "searching") return t("tools.speedTestSearching");
  if (currentPhase.value === "latency") return t("tools.speedTestPingPhase");
  if (currentPhase.value === "download") return t("tools.speedTestDownPhase");
  if (currentPhase.value === "upload") return t("tools.speedTestUpPhase");
  return "";
});

function updateAnimation() {
  const target = liveMbps.value;
  animatedMbps.value += (target - animatedMbps.value) * 0.15;
  animFrameId = requestAnimationFrame(updateAnimation);
}

onMounted(async () => {
  animFrameId = requestAnimationFrame(updateAnimation);

  try {
    const state = await api.getState();
    tunnelUp.value = state.connection.status === "connected";
    const net = await api.getUnderlyingNetwork();
    underlyingType.value = net.type === "cellular" ? "Мобильная сеть" : net.type === "ethernet" ? "Ethernet" : "Wi-Fi";
  } catch {}

  const savedId = loadPinnedServerId();
  if (savedId) {
    selectedServerId.value = savedId;
  }

  // Load Ookla servers in background
  discoverOoklaServers().then((discovered) => {
    if (discovered.length > 0) {
      const existingHosts = new Set(servers.value.map((s) => s.host));
      const combined = [...servers.value];
      for (const d of discovered) {
        if (!existingHosts.has(d.host)) {
          combined.push(d);
          existingHosts.add(d.host);
        }
      }
      servers.value = combined;
    }
  });

  unlisten = await listen<{ phase: string; mbps: number }>("speedtest-tick", (event) => {
    if (abort || !isRunning.value) return;
    const { phase: p, mbps } = event.payload;
    if (p === "download" || p === "upload") {
      liveMbps.value = mbps;
    }
  });
});

onBeforeUnmount(() => {
  abort = true;
  if (animFrameId) cancelAnimationFrame(animFrameId);
  unlisten?.();
});

function selectScope(s: Scope) {
  scope.value = s;
  saveScope(s);
  if (s !== "both" && !isRunning.value) {
    directResult.value = null;
  }
}

function onServerSelectChange() {
  if (selectedServerId.value === "auto") {
    savePinnedServerId(null);
  } else {
    savePinnedServerId(selectedServerId.value);
  }
}

function clearHistory() {
  history.value = [];
  saveHistory([]);
}

async function resolveTarget(viaTunnel: boolean): Promise<SpeedTestServer> {
  const chosen = selectedServerId.value !== "auto"
    ? servers.value.find((s) => s.id === selectedServerId.value)
    : null;
  if (chosen) return chosen;

  // Quick probe candidate servers
  const candidates = servers.value.slice(0, 5);
  let bestServer = candidates[0];
  let bestPing = 999999;

  for (const s of candidates) {
    if (abort) break;
    try {
      const url = getServerLatencyUrl(s);
      const res = await api.speedTestProbeLatency(url, 2, !viaTunnel);
      if (res.pingMs !== null && res.pingMs < bestPing) {
        bestPing = res.pingMs;
        bestServer = s;
      }
    } catch {}
  }

  return bestServer;
}

async function runSpeedTest() {
  if (isRunning.value) return;

  abort = false;
  isRunning.value = true;
  primaryResult.value = null;
  directResult.value = null;
  liveMbps.value = 0.0;
  animatedMbps.value = 0.0;
  currentPing.value = null;
  currentJitter.value = null;
  currentDown.value = null;
  currentUp.value = null;

  // Refresh tunnel status
  try {
    const state = await api.getState();
    tunnelUp.value = state.connection.status === "connected";
  } catch {}

  const runTunnelFirst = tunnelUp.value && scope.value !== "direct";
  hadTunnel.value = runTunnelFirst;

  // 1. Target resolution
  currentPhase.value = "searching";
  const target = await resolveTarget(runTunnelFirst);
  activeServer.value = target;

  if (abort) {
    cleanupRun();
    return;
  }

  // 2. Latency & Jitter probe
  currentPhase.value = "latency";
  let ping: number | null = null;
  let jitter: number | null = null;
  try {
    const lat = await api.speedTestProbeLatency(getServerLatencyUrl(target), 8, !runTunnelFirst);
    ping = lat.pingMs;
    jitter = lat.jitterMs;
    currentPing.value = ping;
    currentJitter.value = jitter;
  } catch {}

  if (abort) {
    cleanupRun();
    return;
  }

  // 3. Multi-stream Download
  currentPhase.value = "download";
  liveMbps.value = 0.0;
  animatedMbps.value = 0.0;
  let downMbps: number | null = null;
  try {
    const downRes = await api.speedTestMultiStreamDownload(8, getServerDownloadUrl(target), !runTunnelFirst);
    downMbps = downRes.mbps;
    currentDown.value = downMbps;
  } catch {}

  if (abort) {
    cleanupRun();
    return;
  }

  // 4. Multi-stream Upload
  currentPhase.value = "upload";
  liveMbps.value = 0.0;
  animatedMbps.value = 0.0;
  let upMbps: number | null = null;
  try {
    const upRes = await api.speedTestMultiStreamUpload(8, getServerUploadUrl(target), !runTunnelFirst);
    upMbps = upRes.mbps;
    currentUp.value = upMbps;
  } catch {}

  const primary: SpeedTestRunResult = {
    downloadMbps: downMbps,
    uploadMbps: upMbps,
    pingMs: ping,
    jitterMs: jitter,
    viaTunnel: runTunnelFirst,
    server: target,
    atMillis: Date.now(),
    path: runTunnelFirst ? "Туннель VPN" : underlyingType.value,
  };
  primaryResult.value = primary;

  // 5. Direct run if "both" was selected
  if (scope.value === "both" && tunnelUp.value && !abort) {
    currentPhase.value = "searching";
    let directTarget = target;
    try {
      const probeDirect = await api.speedTestProbeLatency(getServerLatencyUrl(target), 2, true);
      if (probeDirect.pingMs === null) {
        directTarget = await resolveTarget(false);
      }
    } catch {
      directTarget = await resolveTarget(false);
    }

    if (abort) {
      cleanupRun();
      return;
    }

    // Direct latency
    currentPhase.value = "latency";
    let dirPing: number | null = null;
    let dirJitter: number | null = null;
    try {
      const lat = await api.speedTestProbeLatency(getServerLatencyUrl(directTarget), 6, true);
      dirPing = lat.pingMs;
      dirJitter = lat.jitterMs;
    } catch {}

    if (abort) {
      cleanupRun();
      return;
    }

    // Direct download
    currentPhase.value = "download";
    liveMbps.value = 0.0;
    animatedMbps.value = 0.0;
    let dirDownMbps: number | null = null;
    try {
      const downRes = await api.speedTestMultiStreamDownload(8, getServerDownloadUrl(directTarget), true);
      dirDownMbps = downRes.mbps;
    } catch {}

    if (abort) {
      cleanupRun();
      return;
    }

    // Direct upload
    currentPhase.value = "upload";
    liveMbps.value = 0.0;
    animatedMbps.value = 0.0;
    let dirUpMbps: number | null = null;
    try {
      const upRes = await api.speedTestMultiStreamUpload(8, getServerUploadUrl(directTarget), true);
      dirUpMbps = upRes.mbps;
    } catch {}

    directResult.value = {
      downloadMbps: dirDownMbps,
      uploadMbps: dirUpMbps,
      pingMs: dirPing,
      jitterMs: dirJitter,
      viaTunnel: false,
      server: directTarget,
      atMillis: Date.now(),
      path: underlyingType.value,
    };
  }

  // Save entry in history
  if (primary.downloadMbps !== null || primary.uploadMbps !== null) {
    const entry: SpeedTestHistoryEntry = {
      atMillis: primary.atMillis,
      serverName: hadTunnel.value ? primary.server.sponsor : "",
      downloadMbps: primary.downloadMbps,
      uploadMbps: primary.uploadMbps,
      pingMs: primary.pingMs,
      jitterMs: primary.jitterMs,
      directDownloadMbps: directResult.value?.downloadMbps,
      directUploadMbps: directResult.value?.uploadMbps,
      endpointId: target.id,
      endpointLabel: `${target.sponsor} · ${target.place}`,
      direct: !hadTunnel.value,
    };
    history.value = [entry, ...history.value.slice(0, 29)];
    saveHistory(history.value);
  }

  cleanupRun();
}

function stopTest() {
  abort = true;
  cleanupRun();
}

function cleanupRun() {
  isRunning.value = false;
  currentPhase.value = primaryResult.value ? "done" : "idle";
  liveMbps.value = 0.0;
  animatedMbps.value = 0.0;
}
</script>

<template>
  <ModalSheet :title="t('tools.speedTestTitle')" @close="emit('close')">
    <div class="flex flex-col gap-4 pb-4 pt-1">
      <!-- Start / Stop Button -->
      <button
        v-if="!isRunning"
        type="button"
        class="btn btn-primary w-full py-3.5 text-base font-semibold shadow-md active:scale-[0.98] transition-all flex items-center justify-center gap-2"
        @click="runSpeedTest"
      >
        <AppIcon name="rocket" :size="18" />
        <span>{{ primaryResult ? t('tools.testAgain') : t('tools.speedTestStart') }}</span>
      </button>
      <button
        v-else
        type="button"
        class="btn btn-quiet w-full py-3.5 text-base font-semibold text-danger hover:bg-danger-soft active:scale-[0.98] transition-all flex items-center justify-center gap-2 border border-danger/30"
        @click="stopTest"
      >
        <span class="size-2 rounded-full bg-danger animate-ping" />
        <span>{{ t('tools.speedTestStop') }}</span>
      </button>

      <!-- Separate Status Indicator -->
      <div class="flex items-center justify-center -my-0.5 min-h-[30px]">
        <div
          class="inline-flex items-center gap-2 px-3.5 py-1 rounded-full text-xs font-medium transition-all duration-300"
          :class="isRunning
            ? 'bg-accent/10 text-accent border border-accent/25 shadow-sm shadow-accent/5'
            : primaryResult
              ? 'bg-good/10 text-good border border-good/25'
              : 'bg-card text-muted border border-line/40'"
        >
          <span v-if="isRunning" class="relative flex size-1.5 shrink-0">
            <span class="absolute inline-flex size-full animate-ping rounded-full bg-accent opacity-75" />
            <span class="relative inline-flex size-1.5 rounded-full bg-accent" />
          </span>
          <span v-else-if="primaryResult" class="size-1.5 rounded-full bg-good shrink-0" />
          <span v-else class="size-1.5 rounded-full bg-muted/50 shrink-0" />
          <span class="tracking-wide">{{ phaseText }}</span>
        </div>
      </div>

      <!-- 3 Key Metric Cards: Download, Upload, Ping with live dynamic updates -->
      <div class="grid grid-cols-3 gap-2.5">
        <!-- Download -->
        <div
          class="card p-3 flex flex-col items-center justify-between min-h-[96px] text-center transition-all duration-300"
          :class="isRunning && currentPhase === 'download' ? 'border-accent shadow-[0_0_15px_-3px_var(--color-accent)] bg-accent/5' : ''"
        >
          <div class="flex items-center gap-1 text-xs font-medium text-muted">
            <AppIcon
              name="down"
              :size="13"
              class="text-accent shrink-0"
              :class="isRunning && currentPhase === 'download' ? 'animate-bounce' : ''"
            />
            <span class="truncate">{{ t('tools.speedTestDownload') }}</span>
          </div>
          <div class="my-auto font-mono text-xl font-bold tracking-tight text-accent tabular-nums">
            {{ displayDownload }}
          </div>
          <div class="text-[11px] text-muted font-medium">Мбит/с</div>
        </div>

        <!-- Upload -->
        <div
          class="card p-3 flex flex-col items-center justify-between min-h-[96px] text-center transition-all duration-300"
          :class="isRunning && currentPhase === 'upload' ? 'border-good shadow-[0_0_15px_-3px_#22c55e] bg-good/5' : ''"
        >
          <div class="flex items-center gap-1 text-xs font-medium text-muted">
            <AppIcon
              name="up"
              :size="13"
              class="text-good shrink-0"
              :class="isRunning && currentPhase === 'upload' ? 'animate-bounce' : ''"
            />
            <span class="truncate">{{ t('tools.speedTestUpload') }}</span>
          </div>
          <div class="my-auto font-mono text-xl font-bold tracking-tight text-good tabular-nums">
            {{ displayUpload }}
          </div>
          <div class="text-[11px] text-muted font-medium">Мбит/с</div>
        </div>

        <!-- Ping & Jitter -->
        <div
          class="card p-3 flex flex-col items-center justify-between min-h-[96px] text-center transition-all duration-300"
          :class="isRunning && currentPhase === 'latency' ? 'border-accent shadow-[0_0_15px_-3px_var(--color-accent)] bg-accent/5' : ''"
        >
          <div class="flex items-center gap-1 text-xs font-medium text-muted">
            <AppIcon
              name="check"
              :size="13"
              class="text-muted shrink-0"
              :class="isRunning && currentPhase === 'latency' ? 'animate-spin' : ''"
            />
            <span class="truncate">{{ t('tools.speedTestPing') }}</span>
          </div>
          <div class="my-auto font-mono text-xl font-bold tracking-tight text-ink tabular-nums">
            {{ displayPing }}
          </div>
          <div class="text-[11px] text-muted font-medium">
            {{ displayJitter }}
          </div>
        </div>
      </div>

      <!-- Comparison Banner (if both runs done) -->
      <div v-if="primaryResult && directResult" class="card p-3.5 space-y-2 border border-accent/30 bg-accent/5 text-xs">
        <div class="flex items-center justify-between font-semibold">
          <span class="text-accent flex items-center gap-1">
            <AppIcon name="check" :size="12" />
            {{ t('tools.speedTestScopeTunnel') }}
          </span>
          <span class="font-mono text-ink">
            ↓ {{ formatRate(primaryResult.downloadMbps).value }} &nbsp; ↑ {{ formatRate(primaryResult.uploadMbps).value }} Мбит/с
          </span>
        </div>
        <div class="flex items-center justify-between font-semibold text-muted">
          <span>{{ t('tools.speedTestScopeDirect') }} ({{ directResult.path || underlyingType }})</span>
          <span class="font-mono text-ink">
            ↓ {{ formatRate(directResult.downloadMbps).value }} &nbsp; ↑ {{ formatRate(directResult.uploadMbps).value }} Мбит/с
          </span>
        </div>
        <div v-if="downloadDeltaPercent !== null" class="border-t border-line/60 pt-2 flex items-center justify-between">
          <span class="text-muted">{{ downloadDeltaPercent < 0 ? t('tools.speedTestLoss') : t('tools.speedTestGain') }}</span>
          <span
            class="font-mono font-bold text-xs px-2 py-0.5 rounded"
            :class="downloadDeltaPercent >= 0 ? 'bg-good/15 text-good' : downloadDeltaPercent > -35 ? 'bg-raised text-ink' : 'bg-danger/15 text-danger'"
          >
            {{ downloadDeltaPercent < 0 ? '−' : '+' }}{{ Math.abs(downloadDeltaPercent) }} %
          </span>
        </div>
      </div>

      <!-- Settings / Options Card -->
      <div class="card p-3.5 space-y-3">
        <!-- Mode Selector (if tunnel is up) -->
        <div v-if="tunnelUp" class="space-y-1.5">
          <label class="text-xs font-semibold text-ink">
            {{ t('tools.speedTestScopeLabel') }}
          </label>
          <div class="grid grid-cols-3 gap-1 p-1 rounded-xl bg-surface border border-line">
            <button
              type="button"
              class="py-1.5 text-xs font-medium rounded-lg transition-all text-center"
              :class="scope === 'tunnel' ? 'bg-accent text-white shadow-sm font-semibold' : 'text-muted hover:text-ink'"
              :disabled="isRunning"
              @click="selectScope('tunnel')"
            >
              {{ t('tools.speedTestScopeTunnel') }}
            </button>
            <button
              type="button"
              class="py-1.5 text-xs font-medium rounded-lg transition-all text-center"
              :class="scope === 'direct' ? 'bg-accent text-white shadow-sm font-semibold' : 'text-muted hover:text-ink'"
              :disabled="isRunning"
              @click="selectScope('direct')"
            >
              {{ t('tools.speedTestScopeDirect') }}
            </button>
            <button
              type="button"
              class="py-1.5 text-xs font-medium rounded-lg transition-all text-center"
              :class="scope === 'both' ? 'bg-accent text-white shadow-sm font-semibold' : 'text-muted hover:text-ink'"
              :disabled="isRunning"
              @click="selectScope('both')"
            >
              {{ t('tools.speedTestScopeBoth') }}
            </button>
          </div>
        </div>

        <!-- Target Server Selector -->
        <div class="space-y-1.5">
          <div class="flex items-center justify-between text-xs font-semibold text-ink">
            <span>{{ t('tools.speedTestTargetsLabel') }}</span>
            <span v-if="activeServer && isRunning" class="text-[11px] text-accent font-normal truncate max-w-[180px]">
              {{ activeServer.sponsor }} ({{ activeServer.place }})
            </span>
          </div>
          <select
            v-model="selectedServerId"
            class="input w-full text-xs py-2 px-3 bg-surface border border-line rounded-xl text-ink"
            :disabled="isRunning"
            @change="onServerSelectChange"
          >
            <option value="auto">{{ t('tools.speedTestTargetAuto') }} ({{ t('tools.speedTestTargetAutoSub') }})</option>
            <option v-for="s in servers" :key="s.id" :value="s.id">
              {{ s.sponsor }} — {{ s.place }}
            </option>
          </select>
        </div>
      </div>

      <!-- History -->
      <div v-if="history.length > 0" class="card p-3.5 space-y-2.5">
        <div class="flex items-center justify-between">
          <div class="text-xs font-semibold text-ink">
            {{ t('tools.speedTestHistoryLabel') }}
          </div>
          <button
            type="button"
            class="text-[11px] text-muted hover:text-danger transition-colors flex items-center gap-1"
            @click="clearHistory"
          >
            <AppIcon name="trash" :size="12" />
            <span>{{ t('tools.speedTestClearHistory') }}</span>
          </button>
        </div>

        <div class="divide-y divide-line/50 text-xs">
          <div
            v-for="(item, idx) in history.slice(0, 5)"
            :key="idx"
            class="py-2.5 flex items-center justify-between"
          >
            <div class="flex flex-col truncate max-w-[50%]">
              <span class="font-medium text-ink truncate">
                {{ item.serverName || item.endpointLabel.split('·')[0].trim() || 'Сервер' }}
              </span>
              <span class="text-[10px] text-muted">
                {{ formatClock(item.atMillis) }} · {{ item.direct ? t('tools.speedTestDirect') : t('tools.speedTestViaTunnel') }}
              </span>
            </div>
            <div class="flex items-center gap-2.5 shrink-0 font-mono text-xs">
              <span class="text-accent font-semibold">↓ {{ formatRate(item.downloadMbps).value }}</span>
              <span class="text-good font-semibold">↑ {{ formatRate(item.uploadMbps).value }}</span>
              <span v-if="item.pingMs" class="text-muted text-[11px] min-w-[32px] text-right">{{ item.pingMs }} мс</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </ModalSheet>
</template>
