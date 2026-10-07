<script setup lang="ts">
import { useI18n } from "vue-i18n";

import AppIcon, { type IconName } from "./AppIcon.vue";

const { t } = useI18n();

const TABS: { to: string; icon: IconName; label: string }[] = [
  { to: "/", icon: "power", label: "nav.home" },
  { to: "/servers", icon: "server", label: "nav.servers" },
  { to: "/routing", icon: "route", label: "nav.routing" },
  { to: "/logs", icon: "logs", label: "nav.logs" },
  { to: "/settings", icon: "settings", label: "nav.settings" },
];
</script>

<template>
  <nav class="flex shrink-0 border-t border-line bg-surface pb-[env(safe-area-inset-bottom)]">
    <RouterLink
      v-for="tab in TABS"
      :key="tab.to"
      :to="tab.to"
      class="group flex flex-1 flex-col items-center gap-1 py-2 text-[11px] text-muted transition-colors duration-200 hover:text-ink"
      exact-active-class="is-active !text-accent"
    >
      <span
        class="flex h-7 w-12 items-center justify-center rounded-full transition-[background-color,transform] duration-300 ease-[var(--ease-out)] group-active:scale-90 group-[.is-active]:bg-accent-soft"
      >
        <AppIcon :name="tab.icon" class="transition-transform duration-300 group-[.is-active]:scale-110" />
      </span>
      {{ t(tab.label) }}
    </RouterLink>
  </nav>
</template>
