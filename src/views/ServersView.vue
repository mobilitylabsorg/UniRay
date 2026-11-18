<script setup lang="ts">
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import ImportDialog from "../components/ImportDialog.vue";
import ModalSheet from "../components/ModalSheet.vue";
import ShareDialog from "../components/ShareDialog.vue";
import { api, type Server, type Subscription } from "../lib/api";
import { formatBytes, formatDate, formatDateTime, serverMeta } from "../lib/format";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t, locale } = useI18n();

const importing = ref(false);
/** The import dialog's starting text: a subscription from a uniray:// link. */
const importText = ref("");

watch(
  () => store.pendingImport,
  (url) => {
    if (!url) return;
    importText.value = url;
    importing.value = true;
    store.pendingImport = null;
  },
  { immediate: true },
);

function closeImport() {
  importing.value = false;
  importText.value = "";
}
const sharing = ref<Server | null>(null);
const confirming = ref<{ text: string; action: () => Promise<unknown> } | null>(null);
const updating = ref(new Set<string>());
const updatingAll = ref(false);

type SortMode = "default" | "ping" | "name";
const searchQuery = ref("");
const sortBy = ref<SortMode>("default");

const PROTOCOL_CHIPS = [
  { id: "all", label: "servers.filterAll" },
  { id: "vless", label: "servers.filterVless" },
  { id: "vmess", label: "servers.filterVmess" },
  { id: "trojan", label: "servers.filterTrojan" },
  { id: "shadowsocks", label: "servers.filterShadowsocks" },
  { id: "wireguard", label: "servers.filterWireguard" },
  { id: "hysteria", label: "servers.filterHysteria" },
];
const activeFilter = ref("all");
/** Only the protocols there are servers of; no row at all for just one. */
const chips = computed(() => {
  const present = PROTOCOL_CHIPS.filter(
    (chip) => chip.id === "all" || store.servers.some((server) => server.protocol.toLowerCase().includes(chip.id)),
  );
  return present.length > 2 ? present : [];
});
watch(chips, (next) => {
  if (!next.some((chip) => chip.id === activeFilter.value)) activeFilter.value = "all";
});

function sortServers(list: Server[]): Server[] {
  let filtered = list;
  if (activeFilter.value !== "all") {
    filtered = filtered.filter((s) => s.protocol.toLowerCase().includes(activeFilter.value.toLowerCase()));
  }
  const q = searchQuery.value.trim().toLowerCase();
  if (q) {
    filtered = filtered.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.protocol.toLowerCase().includes(q) ||
        s.address.toLowerCase().includes(q),
    );
  }
  if (sortBy.value === "ping") {
    return [...filtered].sort((a, b) => {
      const aDelay = a.delayMs;
      const bDelay = b.delayMs;
      const aAlive = aDelay !== null && aDelay > 0;
      const bAlive = bDelay !== null && bDelay > 0;
      if (aAlive && bAlive) return aDelay! - bDelay!;
      if (aAlive) return -1;
      if (bAlive) return 1;
      if (aDelay === null && bDelay !== null) return -1;
      if (aDelay !== null && bDelay === null) return 1;
      return 0;
    });
  } else if (sortBy.value === "name") {
    return [...filtered].sort((a, b) => a.name.localeCompare(b.name));
  }
  return filtered;
}

interface Group {
  subscription: Subscription | null;
  servers: Server[];
}

/** Manually added servers first, then one group per subscription. */
const groups = computed<Group[]>(() => {
  const manual = sortServers(store.servers.filter((server) => !server.subscriptionId));
  const result: Group[] = manual.length > 0 ? [{ subscription: null, servers: manual }] : [];
  for (const subscription of store.subscriptions) {
    const servers = sortServers(store.servers.filter((server) => server.subscriptionId === subscription.id));
    if (servers.length > 0 || !searchQuery.value.trim()) {
      result.push({ subscription, servers });
    }
  }
  return result;
});

const fastestServer = computed<Server | null>(() => {
  const alive = store.servers.filter((s) => s.delayMs !== null && s.delayMs > 0);
  if (alive.length === 0) return null;
  return alive.reduce((best, cur) => (cur.delayMs! < best.delayMs! ? cur : best));
});

const selectedId = computed(() => store.selectedServer?.id ?? null);
const testing = computed(() => store.snapshot?.testing ?? false);

function delayClass(delay: number): string {
  if (delay < 0) return "text-danger";
  if (delay < 400) return "text-good";
  return delay < 1000 ? "text-warn" : "text-danger";
}

function usage(subscription: Subscription): string | null {
  const info = subscription.userInfo;
  if (!info) return null;
  const used = formatBytes(info.upload + info.download);
  return info.total > 0
    ? t("servers.traffic", { used, total: formatBytes(info.total) })
    : t("servers.trafficUnlimited", { used });
}

function expiry(subscription: Subscription): { text: string; expired: boolean } | null {
  const expire = subscription.userInfo?.expire;
  if (!expire) return null;
  const expired = expire * 1000 < Date.now();
  const date = formatDate(expire, locale.value);
  return { text: t(expired ? "servers.expired" : "servers.expires", { date }), expired };
}

function testAll() {
  return store.run(() => api.testDelays());
}

function select(server: Server) {
  return store.run(() => api.selectServer(server.id));
}

/** How much of the traffic limit is used, 0..1; null when unlimited. */
function usedShare(subscription: Subscription): number | null {
  const info = subscription.userInfo;
  if (!info || info.total <= 0) return null;
  return Math.min(1, (info.upload + info.download) / info.total);
}

/** The subscription's facts in one line: traffic, expiry, last update, HWID. */
function subscriptionMeta(subscription: Subscription): { text: string; danger?: boolean }[] {
  const parts: { text: string; danger?: boolean }[] = [];
  const used = usage(subscription);
  if (used) parts.push({ text: used });
  const ends = expiry(subscription);
  if (ends) parts.push({ text: ends.text, danger: ends.expired });
  parts.push({
    text: subscription.updatedAt
      ? t("servers.updated", { date: formatDateTime(subscription.updatedAt, locale.value) })
      : t("servers.never"),
  });
  if (subscription.hwidActive) parts.push({ text: t("servers.hwidActive") });
  return parts;
}

async function update(subscription: Subscription) {
  updating.value.add(subscription.id);
  await store.run(() => api.updateSubscription(subscription.id));
  updating.value.delete(subscription.id);
}

function askDeleteServer(server: Server) {
  confirming.value = {
    text: t("servers.deleteConfirm", { name: server.name }),
    action: () => api.deleteServers([server.id]),
  };
}

function askDeleteSubscription(subscription: Subscription) {
  confirming.value = {
    text: t("servers.deleteSubscription", { name: subscription.name }),
    action: () => api.deleteSubscription(subscription.id),
  };
}

async function selectFastest() {
  if (fastestServer.value) {
    await select(fastestServer.value);
    store.toast(t("servers.fastestSelected", { name: fastestServer.value.name, delay: fastestServer.value.delayMs }));
  }
}

async function updateAll() {
  if (updatingAll.value || store.subscriptions.length === 0) return;
  updatingAll.value = true;
  for (const sub of store.subscriptions) {
    updating.value.add(sub.id);
  }
  try {
    await store.run(() => api.updateAllSubscriptions());
  } finally {
    updating.value.clear();
    updatingAll.value = false;
  }
}

async function confirm() {
  const pending = confirming.value;
  confirming.value = null;
  if (pending) await store.run(pending.action);
}
</script>

<template>
  <div class="page md:max-w-3xl">
    <header class="page-header">
      <h1 class="page-title">{{ t("servers.title") }}</h1>
      <button
        v-if="store.subscriptions.length > 1"
        class="btn btn-quiet px-3"
        :disabled="updatingAll"
        :aria-label="t('servers.updateAll')"
        :title="t('servers.updateAll')"
        @click="updateAll"
      >
        <AppIcon name="refresh" :size="18" :class="updatingAll ? 'animate-spin' : ''" />
        <span class="max-[520px]:hidden">{{ t("servers.updateAll") }}</span>
      </button>
      <button
        class="btn btn-quiet px-3"
        :disabled="testing || store.servers.length === 0"
        :aria-label="t('servers.testAll')"
        :title="t('servers.testAll')"
        @click="testAll"
      >
        <AppIcon name="zap" :size="18" :class="testing ? 'animate-pulse' : ''" />
        <span class="max-[400px]:hidden">{{ testing ? t("servers.testing") : t("servers.test") }}</span>
      </button>
      <button class="btn btn-primary" @click="importing = true">
        <AppIcon name="plus" :size="18" />{{ t("servers.import") }}
      </button>
    </header>

    <!-- Finding a server: search and order, the protocols there are. -->
    <div v-if="store.servers.length > 3" class="mb-3 flex items-center gap-2">
      <div class="relative min-w-0 flex-1">
        <AppIcon name="search" :size="16" class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted" />
        <input
          v-model="searchQuery"
          class="field pr-9 pl-9"
          :placeholder="t('servers.searchPlaceholder')"
          :aria-label="t('servers.searchPlaceholder')"
          spellcheck="false"
          autocapitalize="off"
        />
        <button
          v-if="searchQuery"
          class="absolute top-1/2 right-1 flex size-8 -translate-y-1/2 items-center justify-center rounded-lg text-muted transition-colors hover:text-ink"
          :aria-label="t('common.clear')"
          @click="searchQuery = ''"
        >
          <AppIcon name="close" :size="14" />
        </button>
      </div>
      <select v-model="sortBy" class="field w-auto shrink-0 pr-8 pl-3" :aria-label="t('servers.sort')">
        <option value="default">{{ t("servers.sortDefault") }}</option>
        <option value="ping">{{ t("servers.sortPing") }}</option>
        <option value="name">{{ t("servers.sortName") }}</option>
      </select>
    </div>

    <div v-if="chips.length" class="no-scrollbar -mx-1 mb-3 flex items-center gap-1 overflow-x-auto px-1 py-0.5" role="radiogroup">
      <button
        v-for="chip in chips"
        :key="chip.id"
        type="button"
        role="radio"
        :aria-checked="activeFilter === chip.id"
        class="h-7 shrink-0 rounded-full px-3 text-xs font-medium transition-colors duration-200"
        :class="activeFilter === chip.id ? 'bg-accent-soft text-accent' : 'text-muted hover:bg-raised hover:text-ink'"
        @click="activeFilter = chip.id"
      >
        {{ t(chip.label) }}
      </button>
    </div>

    <!-- The fastest server after a ping test, one click away. -->
    <Transition name="pop">
      <div v-if="fastestServer" class="mb-6 flex min-h-11 items-center gap-3 rounded-xl bg-raised py-1.5 pr-1.5 pl-3 text-sm">
        <AppIcon name="rocket" :size="16" class="shrink-0 text-good" />
        <div class="min-w-0 flex-1 truncate">
          <template v-if="fastestServer.id !== selectedId">
            {{ t("servers.fastestAvailable", { name: fastestServer.name, delay: fastestServer.delayMs }) }}
          </template>
          <span v-else class="text-good">{{ t("servers.fastestActive", { delay: fastestServer.delayMs }) }}</span>
        </div>
        <button v-if="fastestServer.id !== selectedId" class="btn btn-sm btn-primary shrink-0" @click="selectFastest">
          {{ t("servers.select") }}
        </button>
      </div>
    </Transition>

    <div v-if="groups.length === 0" class="card rise flex flex-col items-center gap-2 px-6 py-12 text-center">
      <AppIcon name="server" :size="32" class="text-muted" />
      <div class="font-medium">{{ t("servers.empty") }}</div>
      <p class="max-w-sm text-sm text-muted">{{ t("servers.emptyHint") }}</p>
    </div>

    <section v-for="group in groups" :key="group.subscription?.id ?? 'manual'" class="mb-7">
      <div v-if="group.subscription" class="mb-2 px-1">
        <div class="flex items-center gap-1">
          <h2 class="min-w-0 flex-1 truncate font-semibold">{{ group.subscription.name }}</h2>
          <button
            v-if="group.subscription.supportUrl"
            class="icon-btn"
            :aria-label="t('servers.support')"
            :title="t('servers.support')"
            @click="openUrl(group.subscription.supportUrl)"
          >
            <AppIcon name="external" :size="18" />
          </button>
          <button
            class="icon-btn"
            :aria-label="t('servers.update')"
            :title="t('servers.update')"
            :disabled="updating.has(group.subscription.id)"
            @click="update(group.subscription)"
          >
            <AppIcon name="refresh" :size="18" :class="updating.has(group.subscription.id) ? 'animate-spin' : ''" />
          </button>
          <button
            class="icon-btn"
            :aria-label="t('common.delete')"
            :title="t('common.delete')"
            @click="askDeleteSubscription(group.subscription)"
          >
            <AppIcon name="trash" :size="18" />
          </button>
        </div>
        <div class="flex flex-wrap gap-x-1.5 text-xs text-muted">
          <template v-for="(part, index) in subscriptionMeta(group.subscription)" :key="index">
            <span v-if="index > 0" aria-hidden="true">·</span>
            <span :class="part.danger ? 'text-danger' : ''">{{ part.text }}</span>
          </template>
        </div>
        <div v-if="usedShare(group.subscription) !== null" class="mt-1.5 h-1 max-w-48 overflow-hidden rounded-full bg-raised">
          <div
            class="h-full rounded-full"
            :class="usedShare(group.subscription)! > 0.9 ? 'bg-danger' : 'bg-accent'"
            :style="{ width: `${Math.max(2, usedShare(group.subscription)! * 100)}%` }"
          />
        </div>
        <p v-if="group.subscription.announce" class="selectable mt-2 rounded-lg bg-warn-soft px-3 py-2 text-xs text-warn">
          {{ group.subscription.announce }}
        </p>
        <Transition name="pop">
          <p v-if="group.subscription.lastError" class="selectable mt-2 flex gap-1.5 text-xs text-danger">
            <AppIcon name="alert" :size="14" class="mt-0.5 shrink-0" />{{ group.subscription.lastError }}
          </p>
        </Transition>
      </div>
      <div v-else class="section-title">{{ t("servers.manual") }}</div>

      <TransitionGroup tag="ul" name="list" class="card relative divide-y divide-line overflow-hidden" role="radiogroup">
        <li v-for="server in group.servers" :key="server.id" class="group flex items-center bg-surface">
          <button
            role="radio"
            :aria-checked="server.id === selectedId"
            :data-autofocus="server.id === selectedId || undefined"
            class="flex min-w-0 flex-1 items-center gap-3 py-3 pr-2 pl-4 text-left transition-colors hover:bg-raised"
            @click="select(server)"
          >
            <span class="radio-mark" :class="{ 'is-on': server.id === selectedId }">
              <Transition name="pop">
                <AppIcon v-if="server.id === selectedId" name="check" :size="12" />
              </Transition>
            </span>
            <span class="min-w-0 flex-1">
              <span class="block truncate font-medium">{{ server.name }}</span>
              <span class="flex items-center gap-1 text-xs text-muted">
                <span class="truncate">{{ serverMeta(server) }}</span>
                <AppIcon v-if="server.warnings?.length" name="alert" :size="12" class="shrink-0 text-warn" />
              </span>
            </span>
            <Transition name="fade" mode="out-in">
              <span
                v-if="server.delayMs !== null"
                :key="server.delayMs"
                class="shrink-0 text-xs font-medium tabular-nums"
                :class="delayClass(server.delayMs)"
              >
                {{ server.delayMs < 0 ? t("servers.timeout") : `${server.delayMs} ${t("servers.ms")}` }}
              </span>
              <span v-else-if="testing" class="shrink-0 animate-pulse text-xs text-muted">…</span>
            </Transition>
          </button>
          <div class="row-actions flex shrink-0 items-center pr-1">
            <button class="icon-btn" :aria-label="t('servers.share')" :title="t('servers.share')" @click="sharing = server">
              <AppIcon name="qr" :size="18" />
            </button>
            <button
              v-if="!server.subscriptionId"
              class="icon-btn"
              :aria-label="t('common.delete')"
              :title="t('common.delete')"
              @click="askDeleteServer(server)"
            >
              <AppIcon name="trash" :size="18" />
            </button>
          </div>
        </li>
        <li v-if="group.servers.length === 0" key="empty" class="px-4 py-3 text-sm text-muted">{{ t("servers.empty") }}</li>
      </TransitionGroup>
    </section>

    <Transition name="sheet">
      <ImportDialog v-if="importing" :initial="importText" @close="closeImport" />
    </Transition>
    <Transition name="sheet">
      <ShareDialog v-if="sharing" :server="sharing" @close="sharing = null" />
    </Transition>
    <Transition name="sheet">
      <ModalSheet v-if="confirming" :title="t('common.delete')" @close="confirming = null">
        <p class="text-sm">{{ confirming.text }}</p>
        <div class="mt-4 flex gap-2">
          <button class="btn btn-quiet flex-1" @click="confirming = null">{{ t("common.cancel") }}</button>
          <button class="btn btn-danger flex-1" @click="confirm">{{ t("common.delete") }}</button>
        </div>
      </ModalSheet>
    </Transition>
  </div>
</template>
