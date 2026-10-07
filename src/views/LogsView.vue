<script setup lang="ts">
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import ToggleSwitch from "../components/ToggleSwitch.vue";
import type { LogLine } from "../lib/api";
import { formatClock } from "../lib/format";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const showAccess = ref(true);
const list = ref<HTMLElement | null>(null);
/** Follow new lines only while the view is scrolled to the end. */
const pinned = ref(true);

const lines = computed(() => (showAccess.value ? store.logs : store.logs.filter((line) => line.level !== "access")));

const LEVEL_CLASS: Record<LogLine["level"], string> = {
  debug: "text-muted",
  info: "text-ink",
  warning: "text-warn",
  error: "text-danger",
  access: "text-muted",
};

function onScroll() {
  const el = list.value;
  if (el) pinned.value = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
}

async function scrollToEnd() {
  await nextTick();
  if (list.value) list.value.scrollTop = list.value.scrollHeight;
}

onMounted(scrollToEnd);
watch(
  () => lines.value.length,
  () => {
    if (pinned.value) scrollToEnd();
  },
);

async function copyAll() {
  await writeText(lines.value.map((line) => `${formatClock(line.time)} [${line.level}] ${line.text}`).join("\n"));
  store.toast(t("common.copied"));
}
</script>

<template>
  <div class="flex h-full flex-col px-4 py-5">
    <header class="mb-3 flex items-center gap-2">
      <h1 class="flex-1 text-xl font-semibold">{{ t("logs.title") }}</h1>
      <label class="flex items-center gap-2 text-xs text-muted">
        {{ t("logs.showAccess") }}
        <ToggleSwitch v-model="showAccess" :label="t('logs.showAccess')" />
      </label>
      <button
        class="icon-btn"
        :aria-label="t('logs.copyAll')"
        :title="t('logs.copyAll')"
        :disabled="lines.length === 0"
        @click="copyAll"
      >
        <AppIcon name="copy" :size="18" />
      </button>
      <button
        class="icon-btn"
        :aria-label="t('logs.clear')"
        :title="t('logs.clear')"
        :disabled="store.logs.length === 0"
        @click="store.clearLogs()"
      >
        <AppIcon name="trash" :size="18" />
      </button>
    </header>

    <div v-if="lines.length === 0" class="card flex flex-1 flex-col items-center justify-center gap-2 px-6 text-center">
      <AppIcon name="logs" :size="36" class="text-muted" />
      <div class="font-medium">{{ t("logs.empty") }}</div>
      <p class="text-sm text-muted">{{ t("logs.emptyHint") }}</p>
    </div>
    <div
      v-else
      ref="list"
      class="card selectable min-h-0 flex-1 overflow-y-auto p-3 font-mono text-[11px] leading-relaxed"
      @scroll.passive="onScroll"
    >
      <div v-for="line in lines" :key="line.id" class="break-all" :class="LEVEL_CLASS[line.level]">
        <span class="text-muted">{{ formatClock(line.time) }}</span> {{ line.text }}
      </div>
    </div>
  </div>
</template>
