<script setup lang="ts">
import { useId } from "vue";

/**
 * One setting: its name and a one-line hint on the left, the control on the
 * right. A wide control (a text field, a long select) goes under the name
 * on a phone. The control gets `id` from the slot, so the name labels it.
 */
defineProps<{ label: string; hint?: string; wide?: boolean }>();
const id = useId();
</script>

<template>
  <div class="px-4 py-3">
    <div class="flex gap-x-4 gap-y-2" :class="wide ? 'flex-col sm:flex-row sm:items-center' : 'items-center'">
      <div class="min-w-0 flex-1">
        <label :for="id" class="row-label">{{ label }}</label>
        <span v-if="hint" class="row-hint">{{ hint }}</span>
      </div>
      <div class="flex shrink-0 items-center gap-1" :class="wide ? 'sm:w-60' : ''">
        <slot :id="id" />
      </div>
    </div>
    <slot name="below" />
  </div>
</template>
