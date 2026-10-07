<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";

import AppIcon from "./AppIcon.vue";

defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();

function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") emit("close");
}

onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="fixed inset-0 z-40 flex items-end justify-center bg-black/50 sm:items-center" @click.self="emit('close')">
    <div
      class="flex max-h-[90%] w-full max-w-md flex-col rounded-t-3xl bg-surface p-5 pb-[max(1.25rem,env(safe-area-inset-bottom))] sm:rounded-3xl"
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
      <div class="min-h-0 overflow-y-auto">
        <slot />
      </div>
    </div>
  </div>
</template>
