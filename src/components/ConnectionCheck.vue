<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { api } from "../lib/api";
import { flagOf } from "../lib/flag";
import { nameLine, parseServerName } from "../lib/serverName";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";

/** The last check of the connection: does it work, how fast, where it exits. */
const store = useAppStore();
const { t, locale } = useI18n();

const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  ticker = setInterval(() => (now.value = Date.now()), 1000);
});
onBeforeUnmount(() => clearInterval(ticker));

const checking = ref(false);
const check = computed(() => store.connection?.check ?? null);
const serverName = computed(() => store.servers.find((item) => item.id === store.connection?.serverId)?.name ?? null);
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
  if (result.server && result.server !== serverName.value) {
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
</script>

<template>
  <div class="sline">
    <div v-if="check" class="sline-body">
      <div class="sline-title" :class="check.ok ? 'text-accent' : 'text-danger'">
        <AppIcon :name="check.ok ? 'check' : 'alert'" :size="16" class="shrink-0" />
        <span class="truncate">
          {{ check.ok ? t("home.checkOk") : t("home.checkFailed") }}
          <template v-if="check.ok && check.latencyMs !== null"> · {{ t("home.latency", { ms: check.latencyMs }) }}</template>
        </span>
      </div>
      <div class="sline-text">
        <span v-if="!check.ok" class="selectable">{{ check.error }} · </span>
        <template v-else-if="exit">{{ exit }} · </template>
        {{ checkedAgo }}
      </div>
    </div>
    <div v-else class="sline-body text-muted">{{ t("home.checking") }}</div>
    <button class="sline-button" :disabled="checking" :aria-label="t('home.check')" :title="t('home.check')" @click="runCheck">
      <AppIcon name="refresh" :size="20" :class="checking ? 'animate-spin' : ''" />
    </button>
  </div>
</template>
