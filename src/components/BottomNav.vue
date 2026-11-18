<script setup lang="ts">
import { useI18n } from "vue-i18n";

// The app's own icon, the one the window and the launcher show.
import logo from "../../src-tauri/icons/64x64.png";
import { useAppStore } from "../stores/app";
import AppIcon, { type IconName } from "./AppIcon.vue";

/**
 * bar: the tab bar at the bottom of a phone or a narrow window.
 * sidebar: the column on the left of a wide desktop window.
 * rail: the TV's tabs on the left, one per row, in the 10-foot layout.
 */
withDefaults(defineProps<{ variant?: "bar" | "sidebar" | "rail" }>(), { variant: "bar" });
const { t } = useI18n();
const store = useAppStore();

const TABS: { to: string; icon: IconName; label: string }[] = [
  { to: "/", icon: "power", label: "nav.connection" },
  { to: "/routing", icon: "route", label: "nav.routing" },
  { to: "/logs", icon: "logs", label: "nav.logs" },
  { to: "/settings", icon: "settings", label: "nav.settings" },
];
</script>

<template>
  <nav
    :data-nav-rail="variant === 'rail' || undefined"
    :aria-label="t('nav.label')"
    class="flex shrink-0"
    :class="{
      'border-t border-line bg-surface pb-[env(safe-area-inset-bottom)]': variant === 'bar',
      'w-[14.75rem] flex-col gap-1 border-r border-line px-3.5 pt-[max(1.375rem,env(safe-area-inset-top))] pb-5': variant === 'sidebar',
      'w-52 flex-col gap-1 self-start rounded-3xl bg-surface p-2': variant === 'rail',
    }"
  >
    <div v-if="variant === 'sidebar'" class="flex items-center gap-[0.6875rem] px-2.5 pt-0.5 pb-[1.625rem] text-[1.0625rem] font-bold tracking-tight">
      <img :src="logo" alt="" class="size-[2.125rem] shrink-0" draggable="false" />
      UniRay
    </div>
    <RouterLink
      v-for="tab in TABS"
      :key="tab.to"
      :to="tab.to"
      class="group flex items-center text-muted transition-colors duration-200 hover:text-ink"
      :class="
        variant === 'bar'
          ? 'flex-1 flex-col gap-0.5 rounded-xl py-2 text-[11px] font-medium'
          : variant === 'sidebar'
            ? 'gap-[0.8125rem] rounded-xl px-3.5 py-[0.6875rem] text-[0.9375rem] font-medium hover:bg-raised [&.is-active]:bg-accent-soft [&.is-active]:hover:bg-accent-soft'
            : 'gap-3 rounded-2xl px-3 py-2.5 text-sm'
      "
      exact-active-class="is-active !text-accent"
    >
      <span
        class="relative flex items-center justify-center transition-[background-color,transform] duration-300 ease-[var(--ease-out)] group-active:scale-90"
        :class="variant === 'sidebar' ? '' : 'h-7 w-12 rounded-full group-[.is-active]:bg-accent-soft'"
      >
        <AppIcon :name="tab.icon" :size="20" />
        <!-- The connection tab says whether the VPN is on from every page. -->
        <span
          v-if="tab.to === '/' && store.status === 'connected'"
          class="absolute size-[7px] rounded-full bg-accent"
          :class="variant === 'sidebar' ? '-top-1 left-[0.6875rem] ring-2 ring-bg' : 'top-0.5 right-3'"
          aria-hidden="true"
        />
      </span>
      {{ t(tab.label) }}
    </RouterLink>
  </nav>
</template>
