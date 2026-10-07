<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { api, type InstalledApp } from "../lib/api";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";
import ToggleSwitch from "./ToggleSwitch.vue";

const emit = defineEmits<{ close: [] }>();
const store = useAppStore();
const { t } = useI18n();

const apps = ref<InstalledApp[] | null>(null);
const query = ref("");
const showSystem = ref(false);
const selected = ref(new Set(store.settings?.perApp.packages ?? []));

onMounted(async () => {
  apps.value = (await store.run(() => api.listApps())) ?? [];
});

const visible = computed(() => {
  const needle = query.value.trim().toLowerCase();
  return (apps.value ?? []).filter(
    (app) =>
      // A selected app stays visible whatever the filters say.
      (showSystem.value || !app.system || selected.value.has(app.package)) &&
      (!needle || app.label.toLowerCase().includes(needle) || app.package.toLowerCase().includes(needle)),
  );
});

function toggle(name: string) {
  const next = new Set(selected.value);
  if (!next.delete(name)) next.add(name);
  selected.value = next;
}

async function save() {
  const perApp = { mode: store.settings!.perApp.mode, packages: [...selected.value] };
  await store.saveSettings({ perApp });
  emit("close");
}
</script>

<template>
  <ModalSheet :title="t('settings.perAppChoose')" @close="emit('close')">
    <input
      v-model="query"
      class="field selectable"
      :placeholder="t('settings.perAppSearch')"
      spellcheck="false"
      autocapitalize="off"
    />
    <label class="mt-3 flex items-center gap-3 text-sm">
      <span class="flex-1">{{ t("settings.perAppSystem") }}</span>
      <ToggleSwitch v-model="showSystem" :label="t('settings.perAppSystem')" />
    </label>

    <p v-if="apps === null" class="py-8 text-center text-sm text-muted">…</p>
    <ul v-else class="mt-3 divide-y divide-line">
      <li v-for="app in visible" :key="app.package">
        <button
          role="checkbox"
          :aria-checked="selected.has(app.package)"
          class="flex w-full items-center gap-3 py-2.5 text-left"
          @click="toggle(app.package)"
        >
          <span
            class="flex size-5 shrink-0 items-center justify-center rounded-md border-2"
            :class="selected.has(app.package) ? 'border-accent bg-accent text-accent-ink' : 'border-line'"
          >
            <AppIcon v-if="selected.has(app.package)" name="check" :size="12" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-sm font-medium">{{ app.label }}</span>
            <span class="block truncate font-mono text-[11px] text-muted">{{ app.package }}</span>
          </span>
        </button>
      </li>
    </ul>

    <button class="btn btn-primary sticky bottom-0 mt-3 w-full" @click="save">
      {{ t("common.save") }} · {{ selected.size }}
    </button>
  </ModalSheet>
</template>
