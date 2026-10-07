<script setup lang="ts">
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import type { Server } from "../lib/api";
import { renderQr } from "../lib/qr";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";

const props = defineProps<{ server: Server }>();
const emit = defineEmits<{ close: [] }>();
const store = useAppStore();
const { t } = useI18n();

const qr = ref<string | null>(null);

onMounted(async () => {
  if (props.server.link) {
    // A link too long for a QR code still can be copied.
    qr.value = await renderQr(props.server.link).catch(() => null);
  }
});

async function copy() {
  if (!props.server.link) return;
  await writeText(props.server.link);
  store.toast(t("common.copied"));
}
</script>

<template>
  <ModalSheet :title="server.name" @close="emit('close')">
    <template v-if="server.link">
      <img v-if="qr" :src="qr" alt="QR" class="mx-auto w-full max-w-72 rounded-2xl" />
      <p class="selectable mt-4 max-h-24 overflow-y-auto rounded-xl bg-raised p-3 font-mono text-xs break-all text-muted">
        {{ server.link }}
      </p>
      <button class="btn btn-primary mt-3 w-full" @click="copy">
        <AppIcon name="copy" :size="18" />{{ t("common.copy") }}
      </button>
    </template>
    <p v-else class="text-sm text-muted">{{ t("servers.shareUnavailable") }}</p>
    <div v-if="server.warnings?.length" class="mt-4">
      <div class="section-title">{{ t("servers.warnings") }}</div>
      <p v-for="warning in server.warnings" :key="warning" class="selectable flex gap-2 text-xs text-warn">
        <AppIcon name="alert" :size="14" class="mt-0.5 shrink-0" />{{ warning }}
      </p>
    </div>
  </ModalSheet>
</template>
