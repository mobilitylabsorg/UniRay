<script setup lang="ts">
const props = defineProps<{ modelValue: boolean; label: string; disabled?: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();

function toggle() {
  if (props.disabled) return;
  emit("update:modelValue", !props.modelValue);
}
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="modelValue"
    :aria-label="label"
    :disabled="disabled"
    class="relative h-6 w-11 shrink-0 rounded-full transition-colors disabled:opacity-40 disabled:cursor-not-allowed disabled:pointer-events-none"
    :class="modelValue ? 'bg-accent' : 'bg-line'"
    @click="toggle"
  >
    <span
      class="absolute top-0.5 left-0.5 size-5 rounded-full bg-white shadow transition-transform"
      :class="modelValue ? 'translate-x-5' : ''"
    />
  </button>
</template>
