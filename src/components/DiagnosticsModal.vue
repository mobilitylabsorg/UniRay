<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import ModalSheet from "./ModalSheet.vue";
import { api, type DpiReport } from "../lib/api";
import { useAppStore } from "../stores/app";

const emit = defineEmits<{ close: [] }>();
const store = useAppStore();
const { t } = useI18n();

const running = ref(false);
const applying = ref(false);
const report = ref<DpiReport | null>(null);

const canApply = computed(() => {
  if (!report.value) return false;
  return report.value.canApply;
});

const hasBlocked = computed(() => {
  return report.value?.findings.some((f) => f.verdict === "blocked") ?? false;
});

async function runDiagnostics() {
  if (running.value) return;
  running.value = true;
  try {
    report.value = await api.runDpiProbe();
  } catch (e) {
    store.toast(String(e), "error");
  } finally {
    running.value = false;
  }
}

async function applyFixes() {
  if (applying.value) return;
  applying.value = true;
  try {
    await api.applyDpiFixes();
    store.toast(t("tools.diagnosticsApplied"));
    // Re-fetch store settings
    const state = await api.getState();
    store.snapshot = state;
    emit("close");
  } catch (e) {
    store.toast(String(e), "error");
  } finally {
    applying.value = false;
  }
}

onMounted(() => {
  runDiagnostics();
});
</script>

<template>
  <ModalSheet :title="t('tools.diagnosticsTitle')" @close="emit('close')">
    <div class="space-y-4 py-1">
      <!-- Subtitle Description -->
      <div class="rounded-2xl bg-raised p-4">
        <div class="flex items-center justify-between">
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-muted">
              {{ t('tools.diagnosticsTitle') }}
            </div>
            <div class="mt-1 text-sm text-ink">
              {{ t('tools.diagnosticsSubtitle') }}
            </div>
          </div>
          <button
            class="btn btn-quiet py-1.5 px-3 text-xs"
            :disabled="running"
            @click="runDiagnostics"
          >
            <AppIcon name="refresh" :size="14" :class="running ? 'animate-spin' : ''" />
            <span>{{ running ? t('tools.checking') : t('common.refresh') }}</span>
          </button>
        </div>
      </div>

      <!-- Findings List -->
      <div class="divide-y divide-line rounded-2xl border border-line bg-surface overflow-hidden">
        <template v-if="report">
          <div
            v-for="finding in report.findings"
            :key="finding.id"
            class="p-3.5 flex items-start gap-3 text-sm transition-colors"
          >
            <!-- Status Icon -->
            <div
              class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-lg"
              :class="{
                'bg-good/15 text-good': finding.verdict === 'clear',
                'bg-danger/15 text-danger': finding.verdict === 'blocked',
                'bg-raised text-muted': finding.verdict === 'unknown',
              }"
            >
              <AppIcon v-if="finding.verdict === 'clear'" name="check" :size="15" />
              <AppIcon v-else-if="finding.verdict === 'blocked'" name="alert" :size="15" />
              <span v-else class="text-xs font-bold">—</span>
            </div>

            <!-- Finding Content -->
            <div class="min-w-0 flex-1">
              <div class="flex items-center justify-between gap-2">
                <span class="font-medium text-ink">{{ finding.title }}</span>
                <span
                  class="rounded-full px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wider"
                  :class="{
                    'bg-good/10 text-good': finding.verdict === 'clear',
                    'bg-danger/10 text-danger': finding.verdict === 'blocked',
                    'bg-raised text-muted': finding.verdict === 'unknown',
                  }"
                >
                  <template v-if="finding.verdict === 'clear'">OK</template>
                  <template v-else-if="finding.verdict === 'blocked'">DPI</template>
                  <template v-else>?</template>
                </span>
              </div>
              <p class="mt-1 text-xs leading-relaxed text-muted">{{ finding.detail }}</p>
            </div>
          </div>
        </template>

        <!-- Skeleton Loading State -->
        <div v-else class="p-6 text-center text-muted">
          <AppIcon name="refresh" :size="24" class="mx-auto animate-spin text-accent" />
          <div class="mt-2 text-xs font-medium">{{ t('tools.diagnosticsRunning') }}</div>
        </div>
      </div>

      <!-- Clean Network Banner -->
      <div
        v-if="report && !hasBlocked"
        class="card flex items-start gap-3 border border-good/20 bg-good-soft p-3.5"
      >
        <span class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-good/20 text-good">
          <AppIcon name="check" :size="16" />
        </span>
        <div class="text-xs leading-relaxed text-good">
          {{ t('tools.diagnosticsClear') }}
        </div>
      </div>

      <!-- Actionable Advice & Apply Button -->
      <div v-if="report && canApply" class="space-y-3">
        <div class="rounded-2xl border border-amber-500/20 bg-amber-500/10 p-3.5">
          <div class="flex items-center gap-2 text-xs font-semibold text-amber-400">
            <AppIcon name="alert" :size="14" />
            <span>Обнаружена фильтрация провайдера</span>
          </div>
          <p class="mt-1.5 text-xs leading-relaxed text-ink/80">
            Рекомендуется включить фрагментацию TLS (обход ТСПУ), эмуляцию Chrome uTLS и защищённый DoH DNS в туннеле.
          </p>
        </div>

        <button
          class="btn btn-primary w-full py-3"
          :disabled="applying"
          @click="applyFixes"
        >
          <AppIcon name="zap" :size="18" :class="applying ? 'animate-spin' : ''" />
          <span>{{ t('tools.diagnosticsApply') }}</span>
        </button>
      </div>
    </div>
  </ModalSheet>
</template>
