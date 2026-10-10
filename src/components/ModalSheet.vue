<script setup lang="ts">
// Open it inside <Transition name="sheet"> for the slide in and out.
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";

import { onBack } from "../lib/backStack";
import { focusFirst } from "../lib/spatialNav";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";

defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();
const store = useAppStore();
const panel = ref<HTMLElement | null>(null);
const content = ref<HTMLElement | null>(null);

function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") emit("close");
}

// Android's Back key closes the sheet (the topmost first).
let unregister: (() => void) | null = null;
// Where the focus was, to give it back when the sheet closes.
let opener: HTMLElement | null = null;

onMounted(async () => {
  window.addEventListener("keydown", onKey);
  unregister = onBack(() => emit("close"));
  if (store.isTv) {
    opener = document.activeElement as HTMLElement | null;
    await nextTick();
    // The sheet's own controls first, the close button only when it has none.
    const target = content.value?.querySelector("button, input, textarea, select, a[href], [tabindex]") ? content.value : panel.value;
    if (target) focusFirst(target);
  }
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  unregister?.();
  if (opener?.isConnected) opener.focus({ preventScroll: true });
});
</script>

<template>
  <div class="fixed inset-0 z-40 flex items-end justify-center bg-black/50 sm:items-center" @click.self="emit('close')">
    <div
      ref="panel"
      data-focus-scope
      class="sheet-panel flex max-h-[90%] w-full max-w-md flex-col rounded-t-3xl bg-surface p-5 pb-[max(1.25rem,env(safe-area-inset-bottom))] sm:rounded-3xl"
      role="dialog"
      aria-modal="true"
      :aria-label="title"
    >
      <div class="mb-4 flex items-center justify-between">
        <h2 class="text-base font-semibold">{{ title }}</h2>
        <button class="icon-btn" :aria-label="$t('common.close')" @click="emit('close')">
          <AppIcon name="close" />
        </button>
      </div>
      <div ref="content" class="min-h-0 overflow-y-auto">
        <slot />
      </div>
    </div>
  </div>
</template>
