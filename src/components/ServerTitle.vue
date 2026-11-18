<script setup lang="ts">
import type { ServerName } from "../lib/serverName";

/** A server's name with its tag's emoji in front, cut with an ellipsis. */
defineProps<{ name: ServerName }>();
</script>

<template>
  <span class="flex min-w-0 items-center gap-1.5">
    <span v-if="name.emoji" class="shrink-0 text-[0.95em] leading-none">{{ name.emoji }}</span>
    <span class="truncate">{{ name.title }}</span>
  </span>
</template>
