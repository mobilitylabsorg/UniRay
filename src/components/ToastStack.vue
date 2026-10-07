<script setup lang="ts">
import { useAppStore } from "../stores/app";

const store = useAppStore();
</script>

<template>
  <div class="pointer-events-none fixed inset-x-0 top-[max(1rem,env(safe-area-inset-top))] z-50 flex flex-col items-center gap-2 px-4">
    <TransitionGroup
      enter-active-class="transition duration-200"
      enter-from-class="-translate-y-2 opacity-0"
      leave-active-class="transition duration-150"
      leave-to-class="opacity-0"
    >
      <button
        v-for="toast in store.toasts"
        :key="toast.id"
        class="pointer-events-auto max-w-full rounded-xl px-4 py-2.5 text-left text-sm shadow-lg"
        :class="toast.kind === 'error' ? 'bg-danger text-white' : 'bg-ink text-bg'"
        role="status"
        @click="store.dismiss(toast.id)"
      >
        {{ toast.text }}
      </button>
    </TransitionGroup>
  </div>
</template>
