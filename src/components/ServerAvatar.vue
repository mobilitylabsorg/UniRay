<script lang="ts">
// Square flags from flag-icons (MIT), bundled as files and shown in a circle.
const FLAGS = import.meta.glob<string>("../../node_modules/flag-icons/flags/1x1/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
});

function flagUrl(country: string): string | null {
  return FLAGS[`../../node_modules/flag-icons/flags/1x1/${country.toLowerCase()}.svg`] ?? null;
}
</script>

<script setup lang="ts">
import { computed } from "vue";

import type { ServerName } from "../lib/serverName";
import AppIcon from "./AppIcon.vue";

/** A server's round picture: its country's flag, a globe, or a server. */
const props = defineProps<{ name: ServerName }>();

const flag = computed(() => (props.name.country ? flagUrl(props.name.country) : null));
</script>

<template>
  <span
    class="avatar"
    :style="flag ? { backgroundImage: `url(${flag})` } : undefined"
    :title="name.country ?? undefined"
    aria-hidden="true"
  >
    <template v-if="!flag">
      <AppIcon v-if="name.globe" name="globe" :size="18" />
      <AppIcon v-else name="server" :size="15" />
    </template>
  </span>
</template>

<style scoped>
.avatar {
  display: grid;
  flex: none;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  overflow: hidden;
  border-radius: 50%;
  background: var(--color-raised) center / cover no-repeat;
  box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.22);
  color: var(--color-muted);
}
</style>
