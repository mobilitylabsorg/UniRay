<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";
import { api, type ServiceProbeResult } from "../lib/api";

const emit = defineEmits<{ close: [] }>();
const { t } = useI18n();

interface ServiceItem {
  id: string;
  name: string;
  url: string;
}

const SERVICES: ServiceItem[] = [
  { id: "youtube", name: "YouTube", url: "https://www.youtube.com/generate_204" },
  { id: "telegram", name: "Telegram", url: "https://web.telegram.org/favicon.ico" },
  { id: "discord", name: "Discord", url: "https://discord.com/api/v10/gateway" },
  { id: "instagram", name: "Instagram", url: "https://www.instagram.com/favicon.ico" },
  { id: "chatgpt", name: "ChatGPT", url: "https://chatgpt.com/favicon.ico" },
  { id: "x", name: "X (Twitter)", url: "https://x.com/favicon.ico" },
  { id: "steam", name: "Steam", url: "https://api.steampowered.com/ISteamWebAPIUtil/GetServerInfo/v1/" },
  { id: "twitch", name: "Twitch", url: "https://www.twitch.tv/favicon.ico" },
  { id: "spotify", name: "Spotify", url: "https://open.spotify.com/favicon.ico" },
  { id: "rutracker", name: "RuTracker", url: "https://rutracker.org/favicon.ico" },
];

const results = ref<Record<string, { status: "testing" | "ok" | "blocked"; latency?: number; error?: string }>>({});
const testing = ref(false);

const completedCount = computed(() => Object.keys(results.value).filter((k) => results.value[k].status !== "testing").length);
const availableCount = computed(() => Object.values(results.value).filter((r) => r.status === "ok").length);

async function runCheck() {
  if (testing.value) return;
  testing.value = true;
  for (const s of SERVICES) {
    results.value[s.id] = { status: "testing" };
  }

  // Run in parallel chunks of 3 for smooth progress
  for (let i = 0; i < SERVICES.length; i += 3) {
    const chunk = SERVICES.slice(i, i + 3);
    await Promise.all(
      chunk.map(async (service) => {
        try {
          const res: ServiceProbeResult = await api.probeService(service.url);
          if (res.ok) {
            results.value[service.id] = { status: "ok", latency: res.latencyMs };
          } else {
            results.value[service.id] = { status: "blocked", error: res.error ?? t("tools.serviceBlocked") };
          }
        } catch (e) {
          results.value[service.id] = { status: "blocked", error: String(e) };
        }
      }),
    );
  }
  testing.value = false;
}

onMounted(() => {
  void runCheck();
});
</script>

<template>
  <ModalSheet :title="t('tools.serviceCheckTitle')" @close="emit('close')">
    <div class="space-y-4">
      <!-- Summary Card -->
      <div class="rounded-2xl bg-raised p-4">
        <div class="flex items-center justify-between">
          <div>
            <div class="text-xs text-muted">{{ t('tools.serviceCheckStatus') }}</div>
            <div class="mt-0.5 text-lg font-bold">
              <span :class="availableCount === SERVICES.length ? 'text-good' : availableCount > 0 ? 'text-accent' : 'text-danger'">
                {{ availableCount }}
              </span>
              <span class="text-muted"> / {{ SERVICES.length }} {{ t('tools.servicesAvailable') }}</span>
            </div>
          </div>
          <button
            class="btn btn-quiet py-1.5 px-3 text-xs"
            :disabled="testing"
            @click="runCheck"
          >
            <AppIcon name="refresh" :size="14" :class="testing ? 'animate-spin' : ''" />
            <span>{{ testing ? t('tools.checking') : t('common.refresh') }}</span>
          </button>
        </div>
        <div class="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-line">
          <div
            class="h-full bg-accent transition-all duration-300"
            :style="{ width: `${(completedCount / SERVICES.length) * 100}%` }"
          />
        </div>
      </div>

      <!-- Services List -->
      <div class="divide-y divide-line rounded-2xl border border-line bg-surface">
        <div
          v-for="service in SERVICES"
          :key="service.id"
          class="flex items-center justify-between px-3.5 py-2.5 text-sm"
        >
          <div class="flex items-center gap-2.5">
            <div
              class="flex size-7 items-center justify-center rounded-lg font-semibold text-xs"
              :class="
                results[service.id]?.status === 'ok'
                  ? 'bg-good/15 text-good'
                  : results[service.id]?.status === 'blocked'
                  ? 'bg-danger/15 text-danger'
                  : 'bg-raised text-muted'
              "
            >
              {{ service.name[0] }}
            </div>
            <span class="font-medium">{{ service.name }}</span>
          </div>

          <div class="flex items-center gap-2">
            <template v-if="results[service.id]?.status === 'testing'">
              <span class="animate-spin text-muted"><AppIcon name="refresh" :size="14" /></span>
              <span class="text-xs text-muted">{{ t('tools.checking') }}</span>
            </template>
            <template v-else-if="results[service.id]?.status === 'ok'">
              <span class="flex items-center gap-1 rounded-full bg-good/10 px-2 py-0.5 text-xs font-medium text-good">
                <AppIcon name="check" :size="12" />
                <span>{{ results[service.id]?.latency }} ms</span>
              </span>
            </template>
            <template v-else-if="results[service.id]?.status === 'blocked'">
              <span class="flex items-center gap-1 rounded-full bg-danger/10 px-2 py-0.5 text-xs font-medium text-danger">
                <AppIcon name="close" :size="12" />
                <span class="max-w-[130px] truncate">{{ results[service.id]?.error || t('tools.blocked') }}</span>
              </span>
            </template>
            <template v-else>
              <span class="text-xs text-muted">—</span>
            </template>
          </div>
        </div>
      </div>
    </div>
  </ModalSheet>
</template>
