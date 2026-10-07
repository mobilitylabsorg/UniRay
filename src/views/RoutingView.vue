<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import ToggleSwitch from "../components/ToggleSwitch.vue";
import type { Rule, RuleAction, RuleKind, RoutingPreset } from "../lib/api";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const PRESETS: RoutingPreset[] = ["global", "bypassLan", "bypassRu"];
const ACTIONS: RuleAction[] = ["proxy", "direct", "block"];

const rules = computed(() => store.settings?.customRules ?? []);

const kind = ref<RuleKind>("domain");
const value = ref("");
const action = ref<RuleAction>("direct");

const ACTION_CLASS: Record<RuleAction, string> = {
  proxy: "bg-accent-soft text-accent",
  direct: "bg-raised text-ink",
  block: "bg-danger-soft text-danger",
};

function save(next: Rule[]) {
  return store.saveSettings({ customRules: next });
}

async function add() {
  const trimmed = value.value.trim();
  if (!trimmed) return;
  const rule: Rule = { id: crypto.randomUUID(), kind: kind.value, value: trimmed, action: action.value, enabled: true };
  await save([...rules.value, rule]);
  value.value = "";
}

function toggle(rule: Rule, enabled: boolean) {
  return save(rules.value.map((item) => (item.id === rule.id ? { ...item, enabled } : item)));
}

function remove(rule: Rule) {
  return save(rules.value.filter((item) => item.id !== rule.id));
}

function move(index: number, by: number) {
  const next = [...rules.value];
  const [rule] = next.splice(index, 1);
  next.splice(index + by, 0, rule);
  return save(next);
}
</script>

<template>
  <div v-if="store.settings" class="px-4 py-5">
    <h1 class="mb-4 text-xl font-semibold">{{ t("routing.title") }}</h1>

    <div class="section-title">{{ t("routing.preset") }}</div>
    <div class="card mb-6 divide-y divide-line overflow-hidden" role="radiogroup" :aria-label="t('routing.preset')">
      <button
        v-for="preset in PRESETS"
        :key="preset"
        role="radio"
        :aria-checked="store.settings.routingPreset === preset"
        class="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-raised"
        @click="store.saveSettings({ routingPreset: preset })"
      >
        <span
          class="flex size-5 shrink-0 items-center justify-center rounded-full border-2"
          :class="store.settings.routingPreset === preset ? 'border-accent bg-accent text-accent-ink' : 'border-line'"
        >
          <AppIcon v-if="store.settings.routingPreset === preset" name="check" :size="12" />
        </span>
        <span>
          <span class="block font-medium">{{ t(`routing.${preset}`) }}</span>
          <span class="block text-xs text-muted">{{ t(`routing.${preset}Hint`) }}</span>
        </span>
      </button>
    </div>

    <div class="section-title">{{ t("routing.rules") }}</div>
    <div class="card mb-3 p-3">
      <div class="flex gap-2">
        <select v-model="kind" class="field w-24 shrink-0 px-2" :aria-label="t('routing.domain')">
          <option value="domain">{{ t("routing.domain") }}</option>
          <option value="ip">{{ t("routing.ip") }}</option>
        </select>
        <input
          v-model="value"
          class="field selectable min-w-0 font-mono text-xs"
          :placeholder="t(kind === 'domain' ? 'routing.valuePlaceholderDomain' : 'routing.valuePlaceholderIp')"
          spellcheck="false"
          autocapitalize="off"
          @keydown.enter="add"
        />
      </div>
      <div class="mt-2 flex gap-2">
        <select v-model="action" class="field min-w-0 flex-1 px-2" :aria-label="t('routing.preset')">
          <option v-for="item in ACTIONS" :key="item" :value="item">{{ t(`routing.${item}`) }}</option>
        </select>
        <button class="btn btn-primary shrink-0" :disabled="!value.trim()" @click="add">
          <AppIcon name="plus" :size="18" />{{ t("routing.addRule") }}
        </button>
      </div>
    </div>

    <p v-if="rules.length === 0" class="px-1 text-sm text-muted">{{ t("routing.noRules") }}</p>
    <template v-else>
      <ul class="card divide-y divide-line overflow-hidden">
        <li v-for="(rule, index) in rules" :key="rule.id" class="flex items-center gap-2 py-2 pr-1 pl-3">
          <ToggleSwitch
            :model-value="rule.enabled"
            :label="rule.value"
            @update:model-value="(enabled: boolean) => toggle(rule, enabled)"
          />
          <div class="min-w-0 flex-1" :class="rule.enabled ? '' : 'opacity-50'">
            <div class="selectable truncate font-mono text-xs">{{ rule.value }}</div>
            <span class="mt-1 inline-block rounded-md px-1.5 py-0.5 text-[11px]" :class="ACTION_CLASS[rule.action]">
              {{ t(`routing.${rule.action}`) }}
            </span>
          </div>
          <button class="icon-btn" :disabled="index === 0" aria-label="↑" @click="move(index, -1)">
            <AppIcon name="up" :size="16" />
          </button>
          <button class="icon-btn" :disabled="index === rules.length - 1" aria-label="↓" @click="move(index, 1)">
            <AppIcon name="down" :size="16" />
          </button>
          <button class="icon-btn" :aria-label="t('common.delete')" @click="remove(rule)">
            <AppIcon name="trash" :size="16" />
          </button>
        </li>
      </ul>
      <p class="mt-2 px-1 text-xs text-muted">{{ t("routing.rulesHint") }}</p>
    </template>
  </div>
</template>
