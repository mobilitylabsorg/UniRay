<script setup lang="ts">
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";
import { api, toCmdError } from "../lib/api";
import { useAppStore } from "../stores/app";

const emit = defineEmits<{ close: [] }>();
const { t } = useI18n();
const store = useAppStore();

const tab = ref<"export" | "import">("export");
const exportJson = ref("");
const importJson = ref("");
const importMode = ref<"merge" | "replace">("merge");
const loading = ref(false);

async function generateExport() {
  loading.value = true;
  try {
    exportJson.value = await api.exportBackup();
  } catch (err) {
    store.toast(store.describe(toCmdError(err)), "error");
  } finally {
    loading.value = false;
  }
}

async function copyExport() {
  if (!exportJson.value) await generateExport();
  if (exportJson.value) {
    await writeText(exportJson.value).catch(() => navigator.clipboard?.writeText(exportJson.value));
    store.toast(t("common.copied"), "info");
  }
}

function downloadExport() {
  if (!exportJson.value) return;
  const blob = new Blob([exportJson.value], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `uniray-backup-${new Date().toISOString().slice(0, 10)}.json`;
  a.click();
  URL.revokeObjectURL(url);
}

function handleFileUpload(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = (e) => {
    importJson.value = String(e.target?.result || "");
  };
  reader.readAsText(file);
}

async function doImport() {
  if (!importJson.value.trim()) return;
  loading.value = true;
  try {
    await api.importBackup(importJson.value.trim(), importMode.value);
    store.toast(t("tools.backupRestored"), "info");
    emit("close");
  } catch (err) {
    store.toast(store.describe(toCmdError(err)), "error");
  } finally {
    loading.value = false;
  }
}

generateExport();
</script>

<template>
  <ModalSheet :title="t('tools.backupTitle')" @close="emit('close')">
    <div class="space-y-4">
      <!-- Tabs -->
      <div class="flex rounded-xl bg-raised p-1">
        <button
          type="button"
          class="flex-1 rounded-lg py-1.5 text-xs font-semibold transition-colors"
          :class="tab === 'export' ? 'bg-surface text-ink shadow-sm' : 'text-muted'"
          @click="tab = 'export'"
        >
          {{ t('tools.backupExport') }}
        </button>
        <button
          type="button"
          class="flex-1 rounded-lg py-1.5 text-xs font-semibold transition-colors"
          :class="tab === 'import' ? 'bg-surface text-ink shadow-sm' : 'text-muted'"
          @click="tab = 'import'"
        >
          {{ t('tools.backupImport') }}
        </button>
      </div>

      <!-- Export Tab -->
      <div v-if="tab === 'export'" class="space-y-3">
        <div class="text-xs text-muted">
          {{ t('tools.backupExportHint') }}
        </div>
        <textarea
          v-model="exportJson"
          readonly
          rows="6"
          class="field selectable w-full font-mono text-xs leading-relaxed"
          placeholder="..."
        />
        <div class="flex gap-2">
          <button class="btn btn-primary flex-1 py-2 text-xs" @click="copyExport">
            <AppIcon name="copy" :size="14" />
            <span>{{ t('common.copy') }}</span>
          </button>
          <button class="btn btn-quiet flex-1 py-2 text-xs" @click="downloadExport">
            <AppIcon name="down" :size="14" />
            <span>{{ t('tools.downloadFile') }}</span>
          </button>
        </div>
      </div>

      <!-- Import Tab -->
      <div v-else class="space-y-3">
        <div class="text-xs text-muted">
          {{ t('tools.backupImportHint') }}
        </div>
        <textarea
          v-model="importJson"
          rows="6"
          class="field selectable w-full font-mono text-xs leading-relaxed"
          :placeholder="t('tools.backupPastePlaceholder')"
        />

        <div class="flex items-center justify-between text-xs">
          <label class="btn btn-quiet cursor-pointer py-1.5 px-3">
            <AppIcon name="file" :size="14" />
            <span>{{ t('tools.chooseFile') }}</span>
            <input type="file" accept=".json" class="hidden" @change="handleFileUpload" />
          </label>

          <div class="flex items-center gap-2">
            <label class="flex items-center gap-1.5 cursor-pointer">
              <input v-model="importMode" type="radio" value="merge" class="accent-accent" />
              <span>{{ t('tools.modeMerge') }}</span>
            </label>
            <label class="flex items-center gap-1.5 cursor-pointer">
              <input v-model="importMode" type="radio" value="replace" class="accent-accent" />
              <span>{{ t('tools.modeReplace') }}</span>
            </label>
          </div>
        </div>

        <button
          class="btn btn-primary w-full py-2.5 text-xs"
          :disabled="!importJson.trim() || loading"
          @click="doImport"
        >
          <AppIcon name="check" :size="16" />
          <span>{{ loading ? t('common.loading') : t('tools.applyBackup') }}</span>
        </button>
      </div>
    </div>
  </ModalSheet>
</template>
