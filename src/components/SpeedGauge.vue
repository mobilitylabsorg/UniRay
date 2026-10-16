<script setup lang="ts">
import { computed } from "vue";

import { dialFraction, dialLabels, formatRate, GAUGE_START_ANGLE, GAUGE_SWEEP } from "../lib/speedtest";

/**
 * The speed test's dial: a 240° arc open at the bottom, with a scale that
 * follows the peak (dialLabels), the rate in the middle and the phase in the
 * opening. Colours come from the theme, so it reads in light and dark alike.
 */
const props = defineProps<{
  /** The rate to show, in Mbit/s (animated by the caller). */
  mbps: number;
  /** The top of the scale (dialMax of the peak). */
  scaleMax: number;
  /** A test is running: the phase is live. */
  running: boolean;
  /** Download or upload: the arrow by the phase. */
  phase: "idle" | "searching" | "latency" | "download" | "upload" | "done";
  phaseText: string;
}>();

const CENTER = 100;
const RADIUS = 78;

/** A point on a circle around the centre; 0° points right, angles grow clockwise. */
function point(radius: number, angle: number): [number, number] {
  const rad = (angle * Math.PI) / 180;
  return [CENTER + radius * Math.cos(rad), CENTER + radius * Math.sin(rad)];
}

const arc = (() => {
  const [x0, y0] = point(RADIUS, GAUGE_START_ANGLE);
  const [x1, y1] = point(RADIUS, GAUGE_START_ANGLE + GAUGE_SWEEP);
  return `M ${x0} ${y0} A ${RADIUS} ${RADIUS} 0 ${GAUGE_SWEEP > 180 ? 1 : 0} 1 ${x1} ${y1}`;
})();

const fraction = computed(() => dialFraction(props.mbps, props.scaleMax));
const knob = computed(() => point(RADIUS, GAUGE_START_ANGLE + GAUGE_SWEEP * fraction.value));

/** Labels closer than this (of the sweep) to the previous one are left out:
 * the scale's curve crowds 0, 1 and 5 at its start. */
const LABEL_GAP = 0.085;

const ticks = computed(() => {
  let lastLabel = -1;
  return dialLabels(props.scaleMax).map((tick) => {
    const angle = GAUGE_START_ANGLE + GAUGE_SWEEP * tick.fraction;
    const [x0, y0] = point(tick.major ? 87.5 : 88.5, angle);
    const [x1, y1] = point(tick.major ? 93 : 91.5, angle);
    const [lx, ly] = point(63, angle);
    let label = tick.label;
    if (label && tick.fraction - lastLabel < LABEL_GAP) label = "";
    if (label) lastLabel = tick.fraction;
    return { ...tick, label, x0, y0, x1, y1, lx, ly, lit: tick.fraction <= fraction.value + 1e-6 };
  });
});

const rate = computed(() => formatRate(props.mbps));
const active = computed(() => props.mbps > 0 || props.phase === "done");
</script>

<template>
  <div class="relative mx-auto aspect-square w-full max-w-64 select-none">
    <svg class="size-full" viewBox="0 0 200 200" aria-hidden="true">
      <!-- Scale: ticks outside the arc, values inside it. -->
      <g>
        <line
          v-for="(tick, index) in ticks"
          :key="`t${index}`"
          :x1="tick.x0"
          :y1="tick.y0"
          :x2="tick.x1"
          :y2="tick.y1"
          stroke="currentColor"
          :stroke-width="tick.major ? 1.4 : 1"
          stroke-linecap="round"
          :class="tick.lit && active ? 'text-accent' : tick.major ? 'text-muted/60' : 'text-line'"
        />
        <text
          v-for="(tick, index) in ticks.filter((it) => it.label)"
          :key="`l${index}`"
          :x="tick.lx"
          :y="tick.ly"
          text-anchor="middle"
          dominant-baseline="central"
          class="fill-muted text-[8.5px] font-medium tabular-nums"
        >
          {{ tick.label }}
        </text>
      </g>

      <!-- Track and fill. -->
      <path :d="arc" fill="none" stroke="currentColor" stroke-width="9" stroke-linecap="round" class="text-line" />
      <path
        :d="arc"
        fill="none"
        stroke="currentColor"
        stroke-width="9"
        stroke-linecap="round"
        pathLength="1"
        :stroke-dasharray="`${fraction} 1`"
        class="text-accent"
        :class="{ 'opacity-0': !active }"
      />
      <circle
        v-if="active"
        :cx="knob[0]"
        :cy="knob[1]"
        r="6.5"
        class="fill-surface stroke-accent"
        stroke-width="3"
      />
    </svg>

    <!-- The rate, and the phase in the arc's opening. -->
    <div class="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
      <div
        class="text-[2.75rem] font-semibold leading-none tracking-tight tabular-nums"
        :class="active ? 'text-ink' : 'text-muted'"
      >
        {{ rate.value }}
      </div>
      <div class="mt-1.5 text-xs font-medium text-muted">{{ rate.unit }}</div>
    </div>
    <div class="pointer-events-none absolute inset-x-0 bottom-[7%] flex justify-center">
      <div
        class="flex max-w-[70%] items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium"
        :class="running ? 'bg-accent-soft text-accent' : 'text-muted'"
      >
        <svg v-if="phase === 'download' || phase === 'upload'" class="size-3.5 shrink-0" viewBox="0 0 16 16" aria-hidden="true">
          <path
            :d="phase === 'download' ? 'M8 2.5v10M3.5 8.5 8 13l4.5-4.5' : 'M8 13.5v-10M3.5 7.5 8 3l4.5 4.5'"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        <span v-else-if="running" class="size-1.5 shrink-0 animate-pulse rounded-full bg-accent" />
        <span class="truncate">{{ phaseText }}</span>
      </div>
    </div>
  </div>
</template>
