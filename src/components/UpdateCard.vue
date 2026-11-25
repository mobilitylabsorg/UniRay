<script setup lang="ts">
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import SettingRow from "./SettingRow.vue";
import ToggleSwitch from "./ToggleSwitch.vue";
import { api, events, type UpdateProgress } from "../lib/api";
import { useAppStore } from "../stores/app";

/** The settings' rows about new versions: the automatic check, the check
 * button, and what a check found with its notes and the way to install it. */
const store = useAppStore();
const { t } = useI18n();

const checking = ref(false);
const checked = ref(false);
const installing = ref(false);
const progress = ref<UpdateProgress | null>(null);
let unlisten: (() => void) | null = null;

onMounted(async () => {
  unlisten = await events.updateProgress((next) => (progress.value = next));
});
onBeforeUnmount(() => unlisten?.());

async function check() {
  checking.value = true;
  try {
    const found = await store.run(() => api.updateCheck());
    if (found !== undefined) {
      store.update = found;
      checked.value = true;
    }
  } finally {
    checking.value = false;
  }
}

async function install() {
  installing.value = true;
  progress.value = null;
  // On success the app restarts (or the installer takes over on Windows).
  await store.run(() => api.updateInstall());
  installing.value = false;
}

const percent = computed(() => {
  const value = progress.value;
  if (!value?.total) return "";
  return `${Math.min(100, Math.round((value.downloaded / value.total) * 100))}%`;
});
const downloaded = computed(() => progress.value?.total != null && progress.value.downloaded >= progress.value.total);

/** The release notes as lines: headings, list items and paragraphs, with
 * Markdown's emphasis and links reduced to their text. */
const notes = computed(() =>
  (store.update?.notes ?? "")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const plain = line.replace(/\*\*(.+?)\*\*/g, "$1").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1").replace(/`([^`]+)`/g, "$1");
      if (plain.startsWith("#")) return { kind: "heading", text: plain.replace(/^#+\s*/, "") };
      if (/^[-*]\s/.test(plain)) return { kind: "item", text: plain.slice(2) };
      return { kind: "text", text: plain };
    }),
);
</script>

<template>
  <SettingRow v-slot="{ id }" :label="t('update.check')" :hint="t('update.checkHint')">
    <ToggleSwitch
      :id="id"
      :model-value="store.settings?.checkUpdates ?? true"
      :label="t('update.check')"
      @update:model-value="(checkUpdates: boolean) => store.saveSettings({ checkUpdates })"
    />
  </SettingRow>
  <SettingRow
    :label="t('settings.version')"
    :hint="store.update ? t('update.available', { version: store.update.version }) : checked ? t('update.upToDate') : undefined"
  >
    <span class="selectable font-mono text-sm text-muted">{{ store.snapshot?.version }}</span>
    <button type="button" class="btn btn-quiet btn-sm ml-2" :disabled="checking || installing" @click="check">
      {{ checking ? t("update.checking") : t("update.checkNow") }}
    </button>
  </SettingRow>
  <div v-if="store.update" class="px-4 py-3">
    <div class="rounded-xl bg-accent-soft/60 p-3">
      <p class="font-medium">{{ t("update.available", { version: store.update.version }) }}</p>
      <p class="text-xs text-muted">{{ t("update.installed", { current: store.update.current }) }}</p>
      <details v-if="notes.length" class="mt-2 text-sm">
        <summary class="cursor-pointer text-accent">{{ t("update.notes") }}</summary>
        <div class="selectable mt-2 max-h-64 space-y-1 overflow-y-auto pr-1">
          <template v-for="(line, index) in notes" :key="index">
            <p v-if="line.kind === 'heading'" class="pt-1 font-medium">{{ line.text }}</p>
            <p v-else-if="line.kind === 'item'" class="pl-3 -indent-3 text-muted">• {{ line.text }}</p>
            <p v-else class="text-muted">{{ line.text }}</p>
          </template>
        </div>
      </details>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <template v-if="store.update.install">
          <button type="button" class="btn btn-primary btn-sm" :disabled="installing" @click="install">
            {{
              !installing
                ? t("update.install")
                : downloaded
                  ? t("update.installing")
                  : t("update.downloading", { percent: percent })
            }}
          </button>
        </template>
        <template v-else>
          <button type="button" class="btn btn-primary btn-sm" @click="openUrl(store.update.url)">
            {{ t("update.download") }}
          </button>
          <span class="text-xs text-muted">{{ store.isDesktop ? t("update.manualHint") : t("update.apkHint") }}</span>
        </template>
      </div>
    </div>
  </div>
</template>
