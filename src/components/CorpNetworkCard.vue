<script setup lang="ts">
// One corporate network next to the VPN: an OpenVPN profile or a Check Point
// gateway whose networks and domains go through its own tunnel, everything
// else as before. The list and adding networks are CorpNetworks.vue's.
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import ToggleSwitch from "./ToggleSwitch.vue";
import { api, type CorpNetwork, type CorpVia } from "../lib/api";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const props = defineProps<{ corp: CorpNetwork }>();
const corp = computed<CorpNetwork | null>(() => props.corp);
const isCheckPoint = computed(() => corp.value?.kind === "checkPoint");
const status = computed(() => store.connection?.corps[props.corp.id] ?? null);
/** Servers that can stand in front of the corporate one: those Xray runs. */
const fronts = computed(() => store.servers.filter((server) => !["wireguard", "amneziawg", "naive"].includes(server.protocol)));

const fileInput = ref<HTMLInputElement | null>(null);
const importing = ref(false);

// The form, refilled whenever the saved network changes.
const username = ref("");
const password = ref("");
const usePushed = ref(true);
const routes = ref("");
const domains = ref("");
const dns = ref("");
const via = ref("direct");
const saving = ref(false);

// Check Point: the saved gateway's fields.
const gateway = ref("");
/** A certificate just picked: its file name and content in base64. */
const cert = ref<[string, string] | null>(null);
const certPassword = ref("");
const certInput = ref<HTMLInputElement | null>(null);

const certLabel = computed(() => {
  if (cert.value) return cert.value[0];
  if (corp.value?.hasCert) return corp.value.certName || t("corp.cert");
  return t("corp.certPick");
});

async function pickCert(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let at = 0; at < bytes.length; at += 0x8000) binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  cert.value = [file.name, btoa(binary)];
}

function viaKey(value: CorpVia): string {
  return value.kind === "server" ? `server:${value.id}` : value.kind;
}

function viaOf(key: string): CorpVia {
  if (key.startsWith("server:")) return { kind: "server", id: key.slice("server:".length) };
  return key === "main" ? { kind: "main" } : { kind: "direct" };
}

function fill(next: CorpNetwork | null) {
  if (!next) return;
  username.value = next.username;
  password.value = "";
  usePushed.value = next.usePushed;
  routes.value = next.routes.join("\n");
  domains.value = next.domains.join("\n");
  dns.value = next.dns.join("\n");
  via.value = viaKey(next.via);
  gateway.value = next.gateway;
  cert.value = null;
  certPassword.value = "";
}
watch(corp, fill, { immediate: true });

const lines = (text: string) =>
  text
    .split(/[\n,]/)
    .map((line) => line.trim())
    .filter(Boolean);

const dirty = computed(() => {
  const saved = corp.value;
  if (!saved) return false;
  return (
    username.value !== saved.username ||
    password.value !== "" ||
    usePushed.value !== saved.usePushed ||
    lines(routes.value).join() !== saved.routes.join() ||
    lines(domains.value).join() !== saved.domains.join() ||
    lines(dns.value).join() !== saved.dns.join() ||
    via.value !== viaKey(saved.via) ||
    (saved.kind === "checkPoint" && (gateway.value.trim() !== saved.gateway || cert.value !== null || certPassword.value !== ""))
  );
});

async function pick(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  input.value = "";
  if (files.length === 0) return;
  importing.value = true;
  const read = await Promise.all(files.map(async (file): Promise<[string, string]> => [file.name, await file.text()]));
  const done = await store.run(() => api.corpImport(read, props.corp.id));
  importing.value = false;
  if (done !== undefined) store.toast(t("corp.imported"));
}

async function save(enabled = corp.value?.enabled ?? true) {
  if (!corp.value) return;
  saving.value = true;
  await store.run(() =>
    api.corpSave({
      id: props.corp.id,
      enabled,
      username: username.value,
      password: password.value === "" ? null : password.value,
      usePushed: usePushed.value,
      routes: lines(routes.value),
      domains: lines(domains.value),
      dns: lines(dns.value),
      via: viaOf(via.value),
      ...(isCheckPoint.value
        ? { gateway: gateway.value, cert: cert.value, certPassword: certPassword.value === "" ? null : certPassword.value }
        : {}),
    }),
  );
  saving.value = false;
}

async function remove() {
  await store.run(() => api.corpDelete(props.corp.id));
}

const learned = computed(() => {
  const pushed = corp.value?.learned;
  if (!pushed) return [];
  return [...pushed.routes, ...pushed.domains.map((domain) => `*.${domain}`), ...pushed.dns.map((server) => `DNS ${server}`)];
});

/** DNS servers known, but no domain to send to them: nothing would. */
const missingDomains = computed(() => {
  const saved = corp.value;
  if (!saved) return null;
  const pushed = saved.usePushed ? saved.learned : null;
  const servers = [...saved.dns, ...(pushed?.dns ?? [])];
  const anyDomain = saved.domains.length > 0 || (pushed?.domains.length ?? 0) > 0;
  return servers.length > 0 && !anyDomain ? servers.join(", ") : null;
});

/** Why the tunnel is down, in words for the user where it is a known case. */
const errorText = computed(() => {
  const error = status.value?.error;
  if (!error) return null;
  if (error.includes("AUTH_FAILED")) return t("corp.authFailed");
  if (/mac verify failure|PKCS12/i.test(error)) return t("corp.certPasswordWrong");
  return error;
});

/** Where it connects, next to its name. */
const where = computed(() => {
  const info = corp.value?.info;
  if (!info) return null;
  if (isCheckPoint.value) return `${info.host}${info.port === 443 ? "" : `:${info.port}`} · Check Point`;
  return `${info.host}:${info.port} · ${info.proto.toUpperCase()}`;
});

const STATE_CLASS = { connecting: "bg-raised text-muted", up: "bg-accent-soft text-accent", down: "bg-danger-soft text-danger" } as const;

/** The options stay folded until something there needs attention. */
const needsAttention = computed(() => {
  const saved = corp.value;
  if (!saved) return false;
  const noPassword = saved.info?.needsCredentials === true && !saved.hasPassword;
  return noPassword || status.value?.state === "down" || missingDomains.value !== null;
});
const expanded = ref(needsAttention.value);
watch(needsAttention, (attention) => {
  if (attention) expanded.value = true;
});

/** What the options say in short, next to their heading. */
const viaLabel = computed(() => {
  const saved = corp.value?.via;
  if (!saved || saved.kind === "direct" || isCheckPoint.value) return t("corp.viaDirect");
  if (saved.kind === "main") return t("corp.viaMain");
  return store.servers.find((server) => server.id === saved.id)?.name ?? t("corp.viaMain");
});
</script>

<template>
  <input
    ref="fileInput"
    type="file"
    multiple
    accept=".ovpn,.conf,.crt,.pem,.key,.txt"
    class="hidden"
    @change="pick"
  />
  <input ref="certInput" type="file" accept=".p12,.pfx" class="hidden" @change="pickCert" />

  <div v-if="corp" class="card group-card overflow-hidden">
    <div class="row pr-2">
      <ToggleSwitch :model-value="corp.enabled" :label="corp.name" @update:model-value="(on: boolean) => save(on)" />
      <div class="min-w-0 flex-1">
        <div class="truncate font-medium">{{ corp.name }}</div>
        <div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted">
          <span v-if="where" class="selectable truncate font-mono">{{ where }}</span>
          <span v-if="status" class="tag shrink-0" :class="STATE_CLASS[status.state]">
            {{ t(`corp.state.${status.state}`) }}<template v-if="status.state === 'up' && status.address"> · {{ status.address }}</template>
          </span>
        </div>
      </div>
      <button v-if="!isCheckPoint" class="icon-btn" :aria-label="t('corp.replace')" :title="t('corp.replace')" @click="fileInput?.click()">
        <AppIcon name="refresh" :size="16" />
      </button>
      <button class="icon-btn" :aria-label="t('common.delete')" :title="t('common.delete')" @click="remove">
        <AppIcon name="trash" :size="16" />
      </button>
    </div>
    <p v-if="status?.state === 'down' && errorText" class="selectable bg-danger-soft px-4 py-2 text-xs text-danger">
      {{ errorText }}
    </p>

    <button class="row-button py-2.5" :aria-expanded="expanded" @click="expanded = !expanded">
      <span class="flex-1 text-sm font-medium">{{ t("corp.options") }}</span>
      <span class="min-w-0 truncate text-xs text-muted">{{ viaLabel }}</span>
      <AppIcon name="chevron" :size="16" class="shrink-0 text-muted transition-transform duration-200" :class="expanded ? 'rotate-90' : ''" />
    </button>

    <template v-if="expanded">
      <div v-if="isCheckPoint" class="grid gap-2 px-4 py-3">
        <label>
          <span class="mb-1.5 block text-sm font-medium">{{ t("corp.gateway") }}</span>
          <input v-model="gateway" class="field" placeholder="vpn.example.com" autocapitalize="off" spellcheck="false" />
        </label>
        <span class="block text-sm font-medium">{{ t("corp.cert") }}</span>
        <button type="button" class="btn btn-quiet min-w-0 justify-self-start" @click="certInput?.click()">
          <AppIcon name="file" :size="18" /><span class="truncate">{{ certLabel }}</span>
        </button>
        <input
          v-if="cert || corp.hasCert"
          v-model="certPassword"
          class="field"
          type="password"
          :placeholder="corp.hasCertPassword && !cert ? t('corp.certPasswordSaved') : t('corp.certPassword')"
          :aria-label="t('corp.certPassword')"
          autocomplete="off"
        />
        <div class="grid gap-2 sm:grid-cols-2">
          <input
            v-model="username"
            class="field"
            :placeholder="t('corp.username')"
            :aria-label="t('corp.username')"
            autocomplete="username"
            autocapitalize="off"
            spellcheck="false"
          />
          <input
            v-model="password"
            class="field"
            type="password"
            :placeholder="corp.hasPassword ? t('corp.passwordSaved') : t('corp.password')"
            :aria-label="t('corp.password')"
            autocomplete="current-password"
          />
        </div>
      </div>

      <div v-else-if="corp.info?.needsCredentials" class="grid gap-2 px-4 py-3 sm:grid-cols-2">
        <input
          v-model="username"
          class="field"
          :placeholder="t('corp.username')"
          :aria-label="t('corp.username')"
          autocomplete="username"
          autocapitalize="off"
          spellcheck="false"
        />
        <input
          v-model="password"
          class="field"
          type="password"
          :placeholder="corp.hasPassword ? t('corp.passwordSaved') : t('corp.password')"
          :aria-label="t('corp.password')"
          autocomplete="current-password"
        />
      </div>

      <p v-if="isCheckPoint" class="row-hint px-4 py-3">{{ t("corp.directOnly") }}</p>
      <label v-else class="block px-4 py-3">
        <span class="mb-1.5 block text-sm font-medium">{{ t("corp.via") }}</span>
        <select v-model="via" class="field px-2">
          <option value="direct">{{ t("corp.viaDirect") }}</option>
          <option value="main">{{ t("corp.viaMain") }}</option>
          <option v-for="server in fronts" :key="server.id" :value="`server:${server.id}`">{{ server.name }}</option>
        </select>
        <span class="row-hint mt-1.5">{{ t("corp.viaHint") }}</span>
      </label>

      <div class="px-4 py-3">
        <div class="flex items-center gap-3">
          <div class="min-w-0 flex-1">
            <div class="text-sm font-medium">{{ t("corp.usePushed") }}</div>
            <div class="row-hint">{{ t("corp.usePushedHint") }}</div>
          </div>
          <ToggleSwitch v-model="usePushed" :label="t('corp.usePushed')" />
        </div>
        <div v-if="usePushed && learned.length" class="mt-2 flex flex-wrap gap-1">
          <span v-for="item in learned" :key="item" class="selectable tag bg-raised font-mono">{{ item }}</span>
        </div>
        <p v-else-if="usePushed" class="mt-2 text-xs text-muted">{{ t("corp.nothingPushed") }}</p>
      </div>

      <div class="grid gap-3 px-4 py-3">
        <label>
          <span class="mb-1.5 block text-sm font-medium">{{ t("corp.routes") }}</span>
          <textarea v-model="routes" rows="2" class="field selectable h-auto py-2 font-mono text-xs" placeholder="192.168.254.0/24" spellcheck="false" />
        </label>
        <p v-if="missingDomains" class="rounded-lg bg-danger-soft px-3 py-2 text-xs text-danger">
          {{ t("corp.noDomains", { dns: missingDomains }) }}
        </p>
        <label>
          <span class="mb-1.5 block text-sm font-medium">{{ t("corp.domains") }}</span>
          <textarea v-model="domains" rows="2" class="field selectable h-auto py-2 font-mono text-xs" placeholder="corp.example" spellcheck="false" />
        </label>
        <label>
          <span class="mb-1.5 block text-sm font-medium">{{ t("corp.dns") }}</span>
          <textarea v-model="dns" rows="1" class="field selectable h-auto py-2 font-mono text-xs" placeholder="10.0.0.53" spellcheck="false" />
          <span class="row-hint mt-1.5">{{ t("corp.dnsHint") }}</span>
        </label>
        <button class="btn btn-primary justify-self-start" :disabled="!dirty || saving" @click="save()">
          <AppIcon name="check" :size="18" />{{ t("common.save") }}
        </button>
      </div>
    </template>
  </div>
</template>
