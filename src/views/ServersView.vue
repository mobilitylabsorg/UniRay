<script setup lang="ts">
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import ImportDialog from "../components/ImportDialog.vue";
import ModalSheet from "../components/ModalSheet.vue";
import ShareDialog from "../components/ShareDialog.vue";
import { api, type Server, type Subscription } from "../lib/api";
import { formatBytes, formatDate, formatDateTime } from "../lib/format";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t, locale } = useI18n();

const importing = ref(false);
const sharing = ref<Server | null>(null);
const confirming = ref<{ text: string; action: () => Promise<unknown> } | null>(null);
const updating = ref(new Set<string>());
const updatingAll = ref(false);

type SortMode = "default" | "ping" | "name";
const searchQuery = ref("");
const sortBy = ref<SortMode>("default");

function sortServers(list: Server[]): Server[] {
  let filtered = list;
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
  <div class="px-4 py-5">
    <header class="mb-4 flex items-center gap-2">
      <h1 class="flex-1 text-xl font-semibold">{{ t("servers.title") }}</h1>
      <button
        v-if="store.subscriptions.length > 1"
        class="btn btn-quiet px-2.5"
        :disabled="updatingAll"
        :aria-label="t('servers.updateAll')"
        :title="t('servers.updateAll')"
        @click="updateAll"
      >
        <AppIcon name="refresh" :size="18" :class="updatingAll ? 'animate-spin' : ''" />
        <span class="max-[480px]:hidden">{{ t("servers.updateAll") }}</span>
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

    <!-- Fastest Server Highlight Banner -->
    <Transition name="pop">
      <div
        v-if="fastestServer"
        class="card mb-4 flex items-center justify-between gap-3 border border-good/20 bg-good-soft px-3.5 py-2.5"
      >
        <div class="min-w-0 flex-1 flex items-center gap-2.5">
          <span class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-good/20 text-good">
            <AppIcon name="rocket" :size="16" />
          </span>
          <div class="min-w-0 flex-1">
            <div class="truncate text-xs font-semibold text-good">
              {{ fastestServer.name }}
            </div>
            <div class="text-[11px] text-muted">
              {{ t("servers.fastestAvailable", { name: fastestServer.name, delay: fastestServer.delayMs }) }}
            </div>
          </div>
        </div>
        <button
          v-if="fastestServer.id !== selectedId"
          class="btn btn-primary shrink-0 py-1 px-3 text-xs font-medium"
          @click="selectFastest"
        >
          {{ t("servers.select") }}
        </button>
        <span v-else class="shrink-0 flex items-center gap-1 text-xs font-medium text-good">
          <AppIcon name="check" :size="14" />
          {{ t("servers.fastestActive", { delay: fastestServer.delayMs }) }}
        </span>
      </div>
    </Transition>

    <!-- Search & Sort Controls (when there are servers) -->
    <div v-if="store.servers.length > 3" class="mb-4 flex items-center gap-2">
      <div class="relative min-w-0 flex-1">
        <AppIcon name="search" :size="16" class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" />
        <input
          v-model="searchQuery"
          class="field w-full pl-9 pr-8 text-xs h-9"
          :placeholder="t('servers.searchPlaceholder')"
          spellcheck="false"
          autocapitalize="off"
        />
        <button
          v-if="searchQuery"
          class="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted hover:text-ink transition-colors"
          @click="searchQuery = ''"
        >
          <AppIcon name="close" :size="14" />
        </button>
      </div>
      <div class="relative shrink-0">
        <select
          v-model="sortBy"
          class="field h-9 text-xs font-medium pr-7 pl-2.5 cursor-pointer"
          :aria-label="t('servers.sort')"
        >
          <option value="default">{{ t("servers.sortDefault") }}</option>
          <option value="ping">{{ t("servers.sortPing") }}</option>
          <option value="name">{{ t("servers.sortName") }}</option>
        </select>
      </div>
    </div>

    <div v-if="groups.length === 0" class="card rise flex flex-col items-center gap-2 px-6 py-12 text-center">
      <AppIcon name="server" :size="36" class="text-muted" />
      <div class="font-medium">{{ t("servers.empty") }}</div>
      <p class="text-sm text-muted">{{ t("servers.emptyHint") }}</p>
    </div>

    <section v-for="group in groups" :key="group.subscription?.id ?? 'manual'" class="mb-5">
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
        <div class="flex flex-wrap gap-x-3 text-xs text-muted">
          <span v-if="usage(group.subscription)">{{ usage(group.subscription) }}</span>
          <span v-if="expiry(group.subscription)" :class="expiry(group.subscription)!.expired ? 'text-danger' : ''">
            {{ expiry(group.subscription)!.text }}
          </span>
          <span>
            {{
              group.subscription.updatedAt
                ? t("servers.updated", { date: formatDateTime(group.subscription.updatedAt, locale) })
                : t("servers.never")
            }}
          </span>
        </div>
        <p v-if="group.subscription.hwidActive" class="mt-0.5 text-xs text-muted">{{ t("servers.hwidActive") }}</p>
        <p v-if="group.subscription.announce" class="selectable mt-1 text-xs text-warn">
          {{ group.subscription.announce }}
        </p>
        <Transition name="pop">
          <p v-if="group.subscription.lastError" class="selectable mt-1 flex gap-1.5 text-xs text-danger">
            <AppIcon name="alert" :size="14" class="mt-0.5 shrink-0" />{{ group.subscription.lastError }}
          </p>
        </Transition>
      </div>
      <div v-else class="section-title">{{ t("servers.manual") }}</div>

      <TransitionGroup tag="ul" name="list" class="card relative divide-y divide-line overflow-hidden" role="radiogroup">
        <li v-for="server in group.servers" :key="server.id" class="flex items-center bg-surface">
          <button
            role="radio"
            :aria-checked="server.id === selectedId"
            :data-autofocus="server.id === selectedId || undefined"
            class="flex min-w-0 flex-1 items-center gap-3 py-3 pr-1 pl-4 text-left transition-colors hover:bg-raised"
            @click="select(server)"
          >
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full border-2 transition-colors duration-200"
              :class="server.id === selectedId ? 'border-accent bg-accent text-accent-ink' : 'border-line'"
            >
              <Transition name="pop">
                <AppIcon v-if="server.id === selectedId" name="check" :size="12" />
              </Transition>
            </span>
            <span class="min-w-0 flex-1">
              <span class="block truncate font-medium">{{ server.name }}</span>
              <span class="block truncate text-xs text-muted">
                {{ server.protocol }} · {{ server.transport }} · {{ server.security }}
                <AppIcon v-if="server.warnings?.length" name="alert" :size="12" class="inline text-warn" />
              </span>
            </span>
            <Transition name="fade" mode="out-in">
              <span
                v-if="server.delayMs !== null"
                :key="server.delayMs"
                class="shrink-0 font-mono text-xs tabular-nums"
                :class="delayClass(server.delayMs)"
              >
                {{ server.delayMs < 0 ? t("servers.timeout") : `${server.delayMs} ${t("servers.ms")}` }}
              </span>
              <span v-else-if="testing" class="shrink-0 animate-pulse text-xs text-muted">…</span>
            </Transition>
          </button>
          <button class="icon-btn" :aria-label="t('servers.share')" :title="t('servers.share')" @click="sharing = server">
            <AppIcon name="qr" :size="18" />
          </button>
          <button
            v-if="!server.subscriptionId"
            class="icon-btn mr-1"
            :aria-label="t('common.delete')"
            :title="t('common.delete')"
            @click="askDeleteServer(server)"
          >
            <AppIcon name="trash" :size="18" />
          </button>
          <span v-else class="w-1" />
        </li>
        <li v-if="group.servers.length === 0" key="empty" class="px-4 py-3 text-sm text-muted">{{ t("servers.empty") }}</li>
      </TransitionGroup>
    </section>

    <Transition name="sheet">
      <ImportDialog v-if="importing" @close="importing = false" />
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
