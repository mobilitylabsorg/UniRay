<script setup lang="ts">
defineProps<{ modelValue: boolean; label: string; disabled?: boolean }>();
defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="modelValue"
    :aria-label="label"
    :disabled="disabled"
    class="relative h-6 w-11 shrink-0 rounded-full transition-colors disabled:opacity-50"
    :class="modelValue ? 'bg-accent' : 'bg-line'"
    @click="$emit('update:modelValue', !modelValue)"
  >
    <span
      class="absolute top-0.5 left-0.5 size-5 rounded-full bg-white shadow transition-transform"
      :class="modelValue ? 'translate-x-5' : ''"
    />
  </button>
</template>
