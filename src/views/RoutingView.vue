<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import CorpNetworkCard from "../components/CorpNetworkCard.vue";
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
  <div v-if="store.settings" class="page" :class="{ 'xl:max-w-[90rem]': store.snapshot?.corpAvailable }">
    <header class="page-header">
      <h1 class="page-title">{{ t("routing.title") }}</h1>
    </header>

    <!-- A wide window: the mode and the rules on the left, the corporate
         network beside them. -->
    <div
      :class="store.snapshot?.corpAvailable ? 'xl:grid xl:grid-cols-2 xl:grid-rows-[auto_1fr] xl:items-start xl:gap-x-8' : ''"
    >
      <section class="xl:col-start-1">
        <div class="section-title">{{ t("routing.preset") }}</div>
        <div class="card group-card overflow-hidden" role="radiogroup" :aria-label="t('routing.preset')">
          <button
            v-for="preset in PRESETS"
            :key="preset"
            role="radio"
            :aria-checked="store.settings.routingPreset === preset"
            class="row-button"
            @click="store.saveSettings({ routingPreset: preset })"
          >
            <span class="radio-mark" :class="{ 'is-on': store.settings.routingPreset === preset }">
              <Transition name="pop">
                <AppIcon v-if="store.settings.routingPreset === preset" name="check" :size="12" />
              </Transition>
            </span>
            <span class="min-w-0">
              <span class="row-label">{{ t(`routing.${preset}`) }}</span>
              <span class="row-hint">{{ t(`routing.${preset}Hint`) }}</span>
            </span>
          </button>
        </div>
      </section>

      <section v-if="store.snapshot?.corpAvailable" class="xl:col-start-2 xl:row-span-2 xl:row-start-1">
        <CorpNetworkCard />
      </section>

      <section class="xl:col-start-1">
        <div class="section-title">{{ t("routing.rules") }}</div>
        <div class="card overflow-hidden">
          <!-- A new rule: what, which, where to. -->
          <div class="flex flex-wrap gap-2 p-3">
            <select v-model="kind" class="field w-24 shrink-0 px-2" :aria-label="t('routing.kind')">
              <option value="domain">{{ t("routing.domain") }}</option>
              <option value="ip">{{ t("routing.ip") }}</option>
            </select>
            <input
              v-model="value"
              class="field selectable min-w-0 flex-1 basis-40 font-mono text-xs"
              :placeholder="t(kind === 'domain' ? 'routing.valuePlaceholderDomain' : 'routing.valuePlaceholderIp')"
              :aria-label="t(kind === 'domain' ? 'routing.valuePlaceholderDomain' : 'routing.valuePlaceholderIp')"
              spellcheck="false"
              autocapitalize="off"
              @keydown.enter="add"
            />
            <div class="flex min-w-0 grow gap-2 sm:grow-0">
              <select v-model="action" class="field min-w-0 flex-1 px-2 sm:w-32" :aria-label="t('routing.action')">
                <option v-for="item in ACTIONS" :key="item" :value="item">{{ t(`routing.${item}`) }}</option>
              </select>
              <button class="btn btn-primary shrink-0" :disabled="!value.trim()" :title="t('routing.addRule')" @click="add">
                <AppIcon name="plus" :size="18" /><span class="sm:hidden">{{ t("routing.addRule") }}</span><span class="max-sm:hidden">{{ t("common.add") }}</span>
              </button>
            </div>
          </div>

          <p v-if="rules.length === 0" class="border-t border-line px-4 py-3 text-sm text-muted">{{ t("routing.noRules") }}</p>
          <TransitionGroup v-else tag="ul" name="list" class="relative divide-y divide-line border-t border-line">
            <li v-for="(rule, index) in rules" :key="rule.id" class="group flex items-center gap-3 bg-surface py-2 pr-1 pl-4">
              <ToggleSwitch
                :model-value="rule.enabled"
                :label="rule.value"
                @update:model-value="(enabled: boolean) => toggle(rule, enabled)"
              />
              <div class="flex min-w-0 flex-1 items-center gap-2 transition-opacity duration-200" :class="rule.enabled ? '' : 'opacity-50'">
                <span class="selectable min-w-0 truncate font-mono text-xs">{{ rule.value }}</span>
                <span class="tag shrink-0" :class="ACTION_CLASS[rule.action]">{{ t(`routing.${rule.action}`) }}</span>
              </div>
              <div class="flex shrink-0 items-center">
                <button class="icon-btn" :disabled="index === 0" :aria-label="t('routing.moveUp')" :title="t('routing.moveUp')" @click="move(index, -1)">
                  <AppIcon name="up" :size="16" />
                </button>
                <button
                  class="icon-btn"
                  :disabled="index === rules.length - 1"
                  :aria-label="t('routing.moveDown')"
                  :title="t('routing.moveDown')"
                  @click="move(index, 1)"
                >
                  <AppIcon name="down" :size="16" />
                </button>
                <button class="icon-btn" :aria-label="t('common.delete')" :title="t('common.delete')" @click="remove(rule)">
                  <AppIcon name="trash" :size="16" />
                </button>
              </div>
            </li>
          </TransitionGroup>
        </div>
        <p v-if="rules.length" class="mt-2 px-1 text-xs text-muted">{{ t("routing.rulesHint") }}</p>
      </section>
    </div>
  </div>
</template>
