<script setup lang="ts">
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { api, type Server, type Subscription } from "../lib/api";
import { formatBytes, formatDate, formatDateTime } from "../lib/format";
import { dedupeParts, nameLine, parseServerName, protocolName, type ServerName } from "../lib/serverName";
import { useAppStore } from "../stores/app";
import AppIcon from "./AppIcon.vue";
import ImportDialog from "./ImportDialog.vue";
import ModalSheet from "./ModalSheet.vue";
import ServerAvatar from "./ServerAvatar.vue";
import ServerTitle from "./ServerTitle.vue";
import ShareDialog from "./ShareDialog.vue";

/**
 * The servers: added by hand and from each subscription, with search, order,
 * protocol filters, the fastest one after a ping test, and what can be done
 * with each. The right pane of the connection screen, or its lower part.
 */
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

/** Parsed names, once per server. */
const names = computed(() => new Map<string, ServerName>(store.servers.map((server) => [server.id, parseServerName(server.name)])));
const nameOf = (server: Server): ServerName => names.value.get(server.id) ?? parseServerName(server.name);

function kindOf(protocol: string): string {
  return protocol === "balancer" ? t("servers.protoBalancer") : protocolName(protocol);
}

function subtitle(server: Server): string {
  return dedupeParts([...nameOf(server).details, kindOf(server.protocol), server.transport, server.security]).join(" · ");
}

/** A filter per protocol there are servers of; none at all for just one. */
const activeFilter = ref("all");
const chips = computed(() => {
  const protocols = [...new Set(store.servers.map((server) => server.protocol))];
  if (protocols.length < 2) return [];
  return [
    { id: "all", label: t("servers.filterAll") },
    ...protocols.map((protocol) => ({ id: protocol, label: protocol === "balancer" ? t("servers.filterBalancer") : protocolName(protocol) })),
  ];
});
watch(chips, (next) => {
  if (!next.some((chip) => chip.id === activeFilter.value)) activeFilter.value = "all";
});

function sortServers(list: Server[]): Server[] {
  let filtered = list;
  if (activeFilter.value !== "all") {
    filtered = filtered.filter((server) => server.protocol === activeFilter.value);
  }
  const q = searchQuery.value.trim().toLowerCase();
  if (q) {
    filtered = filtered.filter(
      (s) => s.name.toLowerCase().includes(q) || s.protocol.toLowerCase().includes(q) || s.address.toLowerCase().includes(q),
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
    return [...filtered].sort((a, b) => nameOf(a).title.localeCompare(nameOf(b).title));
  }
  return filtered;
}

interface Group {
  subscription: Subscription | null;
  servers: Server[];
  total: number;
}

const filtering = computed(() => searchQuery.value.trim() !== "" || activeFilter.value !== "all");

/** Manually added servers first, then one group per subscription. */
const groups = computed<Group[]>(() => {
  const manualAll = store.servers.filter((server) => !server.subscriptionId);
  const manual = sortServers(manualAll);
  const result: Group[] = manual.length > 0 ? [{ subscription: null, servers: manual, total: manualAll.length }] : [];
  for (const subscription of store.subscriptions) {
    const all = store.servers.filter((server) => server.subscriptionId === subscription.id);
    const servers = sortServers(all);
    if (servers.length > 0 || !filtering.value) {
      result.push({ subscription, servers, total: all.length });
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
  if (delay < 400) return "text-accent";
  return delay < 1000 ? "text-warn" : "text-danger";
}

function usage(subscription: Subscription): string | null {
  const info = subscription.userInfo;
  if (!info) return null;
  const used = formatBytes(info.upload + info.download);
  return info.total > 0 ? t("servers.traffic", { used, total: formatBytes(info.total) }) : t("servers.trafficUnlimited", { used });
}

/** How much of the traffic limit is used, 0..1; null when unlimited. */
function usedShare(subscription: Subscription): number | null {
  const info = subscription.userInfo;
  if (!info || info.total <= 0) return null;
  return Math.min(1, (info.upload + info.download) / info.total);
}

function expiry(subscription: Subscription): { text: string; expired: boolean } | null {
  const expire = subscription.userInfo?.expire;
  if (!expire) return null;
  const expired = expire * 1000 < Date.now();
  const date = formatDate(expire, locale.value);
  return { text: t(expired ? "servers.expired" : "servers.expires", { date }), expired };
}

function updated(subscription: Subscription): string {
  return subscription.updatedAt
    ? t("servers.updated", { date: formatDateTime(subscription.updatedAt, locale.value) })
    : t("servers.never");
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
    text: t("servers.deleteConfirm", { name: nameLine(nameOf(server)) }),
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
  const fastest = fastestServer.value;
  if (fastest) {
    await select(fastest);
    store.toast(t("servers.fastestSelected", { name: nameLine(nameOf(fastest)), delay: fastest.delayMs }));
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
  <section id="server-list" class="list" :class="{ 'is-tv': store.isTv }">
    <div class="list-head">
      <h1>{{ t("servers.title") }}</h1>
      <button
        v-if="store.subscriptions.length > 1"
        class="btn btn-quiet"
        :disabled="updatingAll"
        :aria-label="t('servers.updateAll')"
        :title="t('servers.updateAll')"
        @click="updateAll"
      >
        <AppIcon name="refresh" :size="20" :class="updatingAll ? 'animate-spin' : ''" />
        <span class="max-[560px]:hidden">{{ t("servers.updateAll") }}</span>
      </button>
      <button
        class="btn btn-quiet"
        :disabled="testing || store.servers.length === 0"
        :aria-label="t('servers.testAll')"
        :title="t('servers.testAll')"
        @click="testAll"
      >
        <AppIcon name="zap" :size="20" :class="testing ? 'animate-pulse' : ''" />
        <span class="max-[380px]:hidden">{{ testing ? t("servers.testing") : t("servers.test") }}</span>
      </button>
      <button class="btn btn-primary" @click="importing = true">
        <AppIcon name="plus" :size="20" />{{ t("servers.import") }}
      </button>
    </div>

    <div v-if="store.servers.length > 3" class="toolbar">
      <label class="search">
        <AppIcon name="search" :size="20" class="shrink-0" />
        <input
          v-model="searchQuery"
          data-server-search
          :placeholder="t('servers.searchPlaceholder')"
          :aria-label="t('servers.searchPlaceholder')"
          spellcheck="false"
          autocapitalize="off"
        />
        <button v-if="searchQuery" type="button" class="clear" :aria-label="t('common.clear')" @click="searchQuery = ''">
          <AppIcon name="close" :size="16" />
        </button>
      </label>
      <select v-model="sortBy" class="sort" :aria-label="t('servers.sort')">
        <option value="default">{{ t("servers.sortDefault") }}</option>
        <option value="ping">{{ t("servers.sortPing") }}</option>
        <option value="name">{{ t("servers.sortName") }}</option>
      </select>
    </div>

    <div v-if="chips.length" class="chips" role="radiogroup" :aria-label="t('servers.filter')">
      <button
        v-for="chip in chips"
        :key="chip.id"
        type="button"
        role="radio"
        :aria-checked="activeFilter === chip.id"
        class="chip"
        :class="{ on: activeFilter === chip.id }"
        @click="activeFilter = chip.id"
      >
        {{ chip.label }}
      </button>
    </div>

    <!-- The fastest server after a ping test, one click away. -->
    <Transition name="pop">
      <div v-if="fastestServer && fastestServer.id !== selectedId" class="best">
        <AppIcon name="zap" :size="16" class="shrink-0 text-accent" />
        <span class="text-muted">{{ t("servers.fastest") }}</span>
        <b class="min-w-0 truncate font-medium" :title="fastestServer.name">{{ nameLine(nameOf(fastestServer)) }}</b>
        <span class="tabular-nums text-muted">{{ fastestServer.delayMs }} {{ t("servers.ms") }}</span>
        <button class="pick" @click="selectFastest">{{ t("servers.select") }}</button>
      </div>
    </Transition>

    <div v-if="store.servers.length === 0 && store.subscriptions.length === 0" class="empty card">
      <AppIcon name="server" :size="32" class="text-muted" />
      <div class="font-medium">{{ t("servers.empty") }}</div>
      <p class="max-w-sm text-sm text-muted">{{ t("servers.emptyHint") }}</p>
    </div>
    <p v-else-if="groups.length === 0" class="text-muted">{{ t("servers.nothingFound") }}</p>

    <section v-for="group in groups" :key="group.subscription?.id ?? 'manual'" class="group">
      <template v-if="group.subscription">
        <div class="group-head">
          <h2 class="name">
            <span class="truncate">{{ group.subscription.name }}</span><span class="count">{{ group.total }}</span>
          </h2>
          <button
            v-if="group.subscription.supportUrl"
            class="icon-btn"
            :aria-label="t('servers.support')"
            :title="t('servers.support')"
            @click="openUrl(group.subscription.supportUrl)"
          >
            <AppIcon name="external" :size="16" />
          </button>
          <button
            class="icon-btn"
            :aria-label="t('servers.update')"
            :title="t('servers.update')"
            :disabled="updating.has(group.subscription.id)"
            @click="update(group.subscription)"
          >
            <AppIcon name="refresh" :size="16" :class="updating.has(group.subscription.id) ? 'animate-spin' : ''" />
          </button>
          <button
            class="icon-btn"
            :aria-label="t('servers.deleteSubscriptionShort')"
            :title="t('servers.deleteSubscriptionShort')"
            @click="askDeleteSubscription(group.subscription)"
          >
            <AppIcon name="trash" :size="16" />
          </button>
        </div>
        <div class="sub-meta">
          <span v-if="usage(group.subscription)">{{ usage(group.subscription) }}</span>
          <span v-if="usedShare(group.subscription) !== null" class="bar" aria-hidden="true">
            <i
              :class="usedShare(group.subscription)! > 0.9 ? 'bg-danger' : 'bg-accent'"
              :style="{ width: `${Math.max(3, usedShare(group.subscription)! * 100)}%` }"
            />
          </span>
          <span v-if="expiry(group.subscription)" :class="{ 'text-danger': expiry(group.subscription)!.expired }">
            {{ expiry(group.subscription)!.text }}
          </span>
          <span>{{ updated(group.subscription) }}</span>
          <span v-if="group.subscription.hwidActive">{{ t("servers.hwidActive") }}</span>
        </div>
        <p v-if="group.subscription.announce" class="selectable notice">{{ group.subscription.announce }}</p>
        <Transition name="pop">
          <p v-if="group.subscription.lastError" class="selectable notice error">
            <AppIcon name="alert" :size="14" class="mt-0.5 shrink-0" />{{ group.subscription.lastError }}
          </p>
        </Transition>
      </template>
      <div v-else class="group-head">
        <h2 class="name">
          <span class="truncate">{{ t("servers.manual") }}</span><span class="count">{{ group.total }}</span>
        </h2>
      </div>

      <TransitionGroup tag="ul" name="list" class="rows" role="radiogroup" :aria-label="group.subscription?.name ?? t('servers.manual')">
        <li v-for="server in group.servers" :key="server.id" class="server" :class="{ sel: server.id === selectedId }" :title="server.name">
          <button role="radio" :aria-checked="server.id === selectedId" class="row-main" @click="select(server)">
            <ServerAvatar :name="nameOf(server)" />
            <span class="body">
              <span class="row-title"><ServerTitle :name="nameOf(server)" /></span>
              <span class="row-sub">
                <span class="truncate">{{ subtitle(server) }}</span>
                <AppIcon v-if="server.warnings?.length" name="alert" :size="12" class="shrink-0 text-warn" />
              </span>
            </span>
            <span class="ping tabular-nums">
              <template v-if="server.delayMs !== null">
                <span :class="delayClass(server.delayMs)">
                  {{ server.delayMs < 0 ? t("servers.timeout") : `${server.delayMs} ${t("servers.ms")}` }}
                </span>
              </template>
              <span v-else-if="testing" class="animate-pulse text-muted">…</span>
            </span>
          </button>
          <div class="acts">
            <button class="icon-btn" :aria-label="t('servers.share')" :title="t('servers.share')" @click="sharing = server">
              <AppIcon name="qr" :size="16" />
            </button>
            <button
              v-if="!server.subscriptionId"
              class="icon-btn"
              :aria-label="t('common.delete')"
              :title="t('common.delete')"
              @click="askDeleteServer(server)"
            >
              <AppIcon name="trash" :size="16" />
            </button>
          </div>
        </li>
        <li v-if="group.servers.length === 0" key="empty" class="text-muted">{{ t("servers.empty") }}</li>
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
  </section>
</template>

<style scoped>
/* Sizes follow the approved mock (px / 16 = rem). */
.list {
  container-type: inline-size;
  padding: 1.5rem 1rem 1.75rem;
}

.list-head {
  display: flex;
  align-items: center;
  gap: 0.625rem;
  margin-bottom: 1.375rem;
}
.list-head h1 {
  flex: 1;
  min-width: 0;
  margin: 0;
  font-size: 1.6875rem;
  font-weight: 700;
  letter-spacing: -0.02em;
}
.list-head .btn {
  padding-inline: 1rem;
  border-radius: 0.6875rem;
  font-size: 0.9375rem;
}

.toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 0.625rem;
  margin-bottom: 1rem;
}
.search {
  display: flex;
  flex-basis: 100%;
  align-items: center;
  gap: 0.5rem;
  height: 2.75rem;
  padding: 0 0.875rem;
  border: 1px solid var(--color-line);
  border-radius: 0.75rem;
  background: var(--color-surface);
  color: var(--color-muted);
}
.search:focus-within {
  border-color: var(--color-accent);
}
.search input {
  flex: 1;
  min-width: 0;
  border: 0;
  background: none;
  color: var(--color-ink);
  outline: none;
}
.search input::placeholder {
  color: var(--color-muted);
}
.search .clear {
  display: grid;
  place-items: center;
  width: 1.75rem;
  height: 1.75rem;
  border-radius: 0.5rem;
}
.search .clear:hover {
  color: var(--color-ink);
}
.sort {
  flex: 1;
  height: 2.75rem;
  padding: 0 0.625rem;
  border: 1px solid var(--color-line);
  border-radius: 0.6875rem;
  background: var(--color-surface);
  color: var(--color-ink);
}

.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 0.375rem;
  margin-bottom: 1.125rem;
}
.chip {
  padding: 0.375rem 0.8125rem;
  border-radius: 999px;
  color: var(--color-muted);
  font-size: 0.8125rem;
  font-weight: 500;
}
.chip:hover {
  color: var(--color-ink);
}
.chip.on {
  background: var(--color-accent-soft);
  color: var(--color-accent);
}

.best {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.125rem 0.625rem;
  margin: 0 0.25rem 1.5rem;
  font-size: 0.875rem;
}
.best .pick {
  border-radius: 0.375rem;
  color: var(--color-accent);
  font-weight: 500;
}
.best .pick:hover {
  text-decoration: underline;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
  padding: 3rem 1.5rem;
  text-align: center;
}

.group {
  margin-bottom: 2.25rem;
}
.group-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0 0.25rem 0.625rem;
}
.group-head .name {
  display: flex;
  flex: 1;
  align-items: center;
  gap: 0.5rem;
  min-width: 0;
  margin: 0;
  font-size: 1rem;
  font-weight: 600;
}
.group-head .count {
  color: var(--color-muted);
  font-size: 0.75rem;
  font-weight: 400;
}
.sub-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.25rem 1rem;
  padding: 0 0.25rem 0.75rem;
  color: var(--color-muted);
  font-size: 0.8125rem;
}
.bar {
  width: 7.5rem;
  height: 4px;
  overflow: hidden;
  border-radius: 4px;
  background: var(--color-raised);
}
.bar i {
  display: block;
  height: 100%;
}
.notice {
  display: flex;
  gap: 0.375rem;
  margin: 0 0 0.75rem;
  padding: 0.625rem 0.875rem;
  border-radius: 0.75rem;
  background: var(--color-warn-soft);
  color: var(--color-warn);
  font-size: 0.8125rem;
}
.notice.error {
  background: var(--color-danger-soft);
  color: var(--color-danger);
}

.rows {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 0.625rem;
  margin: 0;
  padding: 0;
  list-style: none;
}
@container (min-width: 840px) {
  .rows {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@container (min-width: 1320px) {
  .rows {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}
.server {
  display: flex;
  align-items: center;
  min-width: 0;
  padding-right: 0.625rem;
  border: 1px solid var(--color-line);
  border-radius: 1rem;
  background: var(--color-surface);
  transition:
    border-color 0.15s,
    background-color 0.15s;
}
.server:hover {
  border-color: color-mix(in srgb, var(--color-accent) 45%, var(--color-line));
}
.server.sel {
  border-color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent-soft) 55%, var(--color-surface));
}
.row-main {
  display: flex;
  flex: 1;
  align-items: center;
  gap: 0.875rem;
  min-width: 0;
  padding: 0.875rem 0.5rem 0.875rem 1rem;
  border-radius: 1rem;
  text-align: left;
}
.body {
  flex: 1;
  min-width: 0;
}
.row-title {
  display: block;
  font-weight: 500;
}
.row-sub {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  margin-top: 1px;
  color: var(--color-muted);
  font-size: 0.8125rem;
}
.ping {
  flex: none;
  min-width: 3.375rem;
  font-size: 0.8125rem;
  font-weight: 500;
  text-align: right;
}
.acts {
  display: flex;
  flex: none;
}
.acts .icon-btn,
.group-head .icon-btn {
  width: 2rem;
  height: 2rem;
}

/* Wide: the right pane. */
@media (min-width: 900px) {
  .list:not(.is-tv) {
    padding: 2.5rem 3rem 3.5rem;
  }
  .list:not(.is-tv) .toolbar {
    flex-wrap: nowrap;
  }
  .list:not(.is-tv) .search {
    flex: 1;
  }
  .list:not(.is-tv) .sort {
    flex: none;
  }
}
</style>
