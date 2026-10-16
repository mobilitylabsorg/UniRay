<script setup lang="ts">
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { api, events, toCmdError, type ImportReport } from "../lib/api";
import { decodeQrImage, renderQr } from "../lib/qr";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";

/** Text to start with: a subscription a uniray:// link offered. */
const props = defineProps<{ initial?: string }>();
const emit = defineEmits<{ close: [] }>();
const store = useAppStore();
const { t } = useI18n();

const text = ref(props.initial ?? "");
const busy = ref(false);
const fileInput = ref<HTMLInputElement | null>(null);
const confInput = ref<HTMLInputElement | null>(null);

function append(value: string) {
  text.value = text.value.trim() ? `${text.value.trim()}\n${value}` : value;
}

async function paste() {
  // An empty or non-text clipboard is not worth an error message.
  const clip = await readText().catch(() => "");
  if (clip) text.value = clip;
}

async function onImage(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  const decoded = await decodeQrImage(file).catch(() => null);
  if (decoded) {
    append(decoded);
  } else {
    store.toast(t("servers.noQr"), "error");
  }
}

/** A WireGuard/AmneziaWG `.conf`, or any text file of links. */
async function onFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  input.value = "";
  for (const file of files) {
    const content = await file.text().catch(() => "");
    if (content.trim()) append(content.trim());
  }
}

async function submit() {
  busy.value = true;
  const report = await store.run(() => api.import(text.value));
  busy.value = false;
  if (report) done(report);
}

function done(report: ImportReport) {
  store.toast(t("servers.imported", { count: report.added }));
  if (report.errors.length > 0) {
    store.toast(`${t("servers.skipped", { count: report.errors.length })}: ${report.errors[0]}`, "error");
  }
  emit("close");
}

// TV: a link typed with the remote is no fun, so the phone sends it. A page
// on the local network, up while this dialog is open, takes it; its address
// is the QR code. Whatever it imports arrives as a `handoff` event.
const handoffQr = ref<string | null>(null);
const handoffUrl = ref<string | null>(null);
const handoffError = ref<string | null>(null);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  if (!store.isTv) return;
  unlisten = await events.handoff(done);
  try {
    handoffUrl.value = await api.handoffStart();
    handoffQr.value = await renderQr(handoffUrl.value);
  } catch (err) {
    handoffError.value = store.describe(toCmdError(err));
  }
});
onBeforeUnmount(() => {
  unlisten?.();
  if (store.isTv) void api.handoffStop();
});
</script>

<template>
  <ModalSheet :title="t('servers.importTitle')" @close="emit('close')">
    <div v-if="store.isTv" class="mb-4 flex items-center gap-4 rounded-2xl bg-raised p-3">
      <img v-if="handoffQr" :src="handoffQr" alt="" class="size-36 shrink-0 rounded-xl" />
      <div class="min-w-0 text-sm">
        <div class="font-medium">{{ t("servers.fromPhone") }}</div>
        <div v-if="handoffError" class="mt-1 text-xs text-danger">{{ handoffError }}</div>
        <template v-else>
          <div class="mt-1 text-xs text-muted">{{ t("servers.fromPhoneHint") }}</div>
          <div v-if="handoffUrl" class="selectable mt-2 font-mono text-xs break-all">{{ handoffUrl }}</div>
        </template>
      </div>
    </div>
    <textarea
      v-model="text"
      class="field selectable h-36 resize-none py-2 font-mono text-xs leading-relaxed"
      :placeholder="t('servers.importPlaceholder')"
      spellcheck="false"
      autocapitalize="off"
      autocomplete="off"
    />
    <div class="mt-3 flex gap-2">
      <button class="btn btn-quiet flex-1" @click="paste">
        <AppIcon name="clipboard" :size="18" />{{ t("servers.paste") }}
      </button>
      <button class="btn btn-quiet flex-1" @click="fileInput?.click()">
        <AppIcon name="image" :size="18" />{{ t("servers.fromImage") }}
      </button>
      <button class="btn btn-quiet flex-1" @click="confInput?.click()">
        <AppIcon name="file" :size="18" />{{ t("servers.fromFile") }}
      </button>
      <input ref="fileInput" type="file" accept="image/*" class="hidden" @change="onImage" />
      <input ref="confInput" type="file" accept=".conf,.txt,.json,.yaml,.yml,text/plain,application/json,text/yaml" multiple class="hidden" @change="onFile" />
    </div>
    <button class="btn btn-primary mt-3 w-full" :disabled="busy || !text.trim()" @click="submit">
      {{ t("common.add") }}
    </button>
  </ModalSheet>
</template>
