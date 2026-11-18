<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import { formatBytes, formatSpeed } from "../lib/format";
import { useAppStore } from "../stores/app";

/**
 * The traffic of the last minute: the speeds and totals, download as an
 * area and upload as a dashed line. Its height comes from --chart-height.
 */
const store = useAppStore();
const { t } = useI18n();

const WIDTH = 340;
const HEIGHT = 72;
const POINTS = 60;
const chart = computed(() => {
  const { down, up } = store.history;
  const max = Math.max(1, ...down, ...up) * 1.15;
  const points = (values: number[]) => {
    const offset = POINTS - values.length;
    return values.map((value, index) => [((index + offset) / (POINTS - 1)) * WIDTH, HEIGHT - (value / max) * (HEIGHT - 4) - 2]);
  };
  const path = (list: number[][]) => (list.length ? "M" + list.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join("L") : "");
  const downPoints = points(down);
  const area = downPoints.length
    ? `${path(downPoints)}L${downPoints[downPoints.length - 1][0].toFixed(1)},${HEIGHT}L${downPoints[0][0].toFixed(1)},${HEIGHT}Z`
    : "";
  return { down: path(downPoints), up: path(points(up)), area };
});
</script>

<template>
  <div class="traffic">
    <div class="head">
      <div class="min-w-0">
        <div class="big tabular-nums">{{ formatSpeed(store.traffic.downSpeed) }}</div>
        <div class="small text-muted">{{ t("home.chartDown", { total: formatBytes(store.traffic.down) }) }}</div>
      </div>
      <div class="min-w-0 text-right">
        <div class="font-semibold tabular-nums">{{ formatSpeed(store.traffic.upSpeed) }}</div>
        <div class="small text-muted">{{ t("home.chartUp", { total: formatBytes(store.traffic.up) }) }}</div>
      </div>
    </div>
    <div>
      <svg class="chart" :viewBox="`0 0 ${WIDTH} ${HEIGHT}`" preserveAspectRatio="none" aria-hidden="true">
        <defs>
          <linearGradient id="chart-down" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="var(--color-accent)" stop-opacity="0.35" />
            <stop offset="1" stop-color="var(--color-accent)" stop-opacity="0" />
          </linearGradient>
        </defs>
        <path :d="chart.area" fill="url(#chart-down)" />
        <path :d="chart.down" fill="none" stroke="var(--color-accent)" stroke-width="2" vector-effect="non-scaling-stroke" />
        <path :d="chart.up" fill="none" stroke="var(--color-up)" stroke-width="1.5" stroke-dasharray="3 3" vector-effect="non-scaling-stroke" />
      </svg>
      <div class="legend text-muted">
        <span><i class="bg-accent" />{{ t("home.legendDown") }}</span>
        <span><i class="bg-up" />{{ t("home.legendUp") }}</span>
        <span class="ml-auto">{{ t("home.lastMinute") }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.traffic {
  display: flex;
  flex-direction: column;
  gap: var(--chart-gap, 1rem);
}
.head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 1rem;
}
.big {
  font-size: 1.5rem;
  font-weight: 650;
  letter-spacing: -0.02em;
}
.small {
  font-size: 0.75rem;
}
.chart {
  display: block;
  width: 100%;
  height: var(--chart-height, 5.25rem);
}
.legend {
  display: flex;
  gap: 1rem;
  font-size: 0.75rem;
}
.legend i {
  display: inline-block;
  width: 0.5rem;
  height: 0.5rem;
  margin-right: 0.3125rem;
  border-radius: 50%;
  vertical-align: 1px;
}
</style>
