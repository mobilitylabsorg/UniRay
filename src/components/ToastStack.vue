<script setup lang="ts">
import { useAppStore } from "../stores/app";

const store = useAppStore();
</script>

<template>
  <div class="pointer-events-none fixed inset-x-0 top-[max(1rem,env(safe-area-inset-top))] z-50 flex flex-col items-center gap-2 px-4">
    <TransitionGroup
      enter-active-class="transition duration-300 ease-[var(--ease-out)]"
      enter-from-class="-translate-y-3 scale-95 opacity-0"
      leave-active-class="transition duration-150 ease-[var(--ease-in)]"
      leave-to-class="scale-95 opacity-0"
      move-class="transition-transform duration-300"
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
