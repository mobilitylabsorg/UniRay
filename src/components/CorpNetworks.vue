<script setup lang="ts">
// The corporate networks next to the VPN: a card for each (any number of
// them can be on at once, each with its own tunnel) and the ways to add one,
// an OpenVPN profile or a Check Point gateway.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "./AppIcon.vue";
import CorpNetworkCard from "./CorpNetworkCard.vue";
import { api } from "../lib/api";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const corps = computed(() => store.snapshot?.corps ?? []);
const kinds = computed(() => store.snapshot?.corpKinds ?? []);

const fileInput = ref<HTMLInputElement | null>(null);
const certInput = ref<HTMLInputElement | null>(null);
const importing = ref(false);

async function pick(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  input.value = "";
  if (files.length === 0) return;
  importing.value = true;
  const read = await Promise.all(files.map(async (file): Promise<[string, string]> => [file.name, await file.text()]));
  const done = await store.run(() => api.corpImport(read, null));
  importing.value = false;
  if (done !== undefined) store.toast(t("corp.imported"));
}

// A new Check Point gateway.
const setupOpen = ref(false);
const name = ref("");
const gateway = ref("");
/** The certificate picked: its file name and content in base64. */
const cert = ref<[string, string] | null>(null);
const certPassword = ref("");
const username = ref("");
const password = ref("");
const saving = ref(false);

function openSetup() {
  name.value = "";
  gateway.value = "";
  cert.value = null;
  certPassword.value = "";
  username.value = "";
  password.value = "";
  setupOpen.value = true;
}

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

async function addCheckPoint() {
  saving.value = true;
  const done = await store.run(() =>
    api.corpCheckPoint({
      id: null,
      name: name.value,
      gateway: gateway.value,
      cert: cert.value,
      certPassword: certPassword.value === "" ? null : certPassword.value,
      username: username.value,
      password: password.value === "" ? null : password.value,
    }),
  );
  saving.value = false;
  if (done !== undefined) {
    setupOpen.value = false;
    store.toast(t("corp.added"));
  }
}
</script>

<template>
  <div class="section-title">{{ corps.length > 1 ? t("corp.titleMany") : t("corp.title") }}</div>
  <input ref="fileInput" type="file" multiple accept=".ovpn,.conf,.crt,.pem,.key,.txt" class="hidden" @change="pick" />
  <input ref="certInput" type="file" accept=".p12,.pfx" class="hidden" @change="pickCert" />

  <div v-for="corp in corps" :key="corp.id" class="mb-3">
    <CorpNetworkCard :corp="corp" />
  </div>

  <div class="card mb-7 p-4">
    <p v-if="corps.length === 0" class="mb-3 text-sm text-muted">{{ t("corp.intro") }}</p>
    <template v-if="!setupOpen">
      <div class="flex flex-wrap gap-2">
        <button v-if="kinds.includes('openVpn')" class="btn btn-quiet" :disabled="importing" @click="fileInput?.click()">
          <AppIcon name="file" :size="18" />{{ t("corp.import") }}
        </button>
        <button v-if="kinds.includes('checkPoint')" class="btn btn-quiet" @click="openSetup">
          <AppIcon name="plus" :size="18" />{{ t("corp.checkPoint") }}
        </button>
      </div>
      <p class="mt-2 text-xs text-muted">
        <template v-if="kinds.includes('openVpn')">{{ t("corp.importHint") }}. </template>{{ t("corp.manyHint") }}
      </p>
    </template>
    <form v-else class="grid gap-2" @submit.prevent="addCheckPoint">
      <p class="text-xs text-muted">{{ t("corp.checkPointHint") }}</p>
      <input
        v-model="gateway"
        class="field"
        placeholder="vpn.example.com"
        :aria-label="t('corp.gateway')"
        autocapitalize="off"
        spellcheck="false"
        required
      />
      <input v-model="name" class="field" :placeholder="t('corp.name')" :aria-label="t('corp.name')" />
      <button type="button" class="btn btn-quiet min-w-0 justify-self-start" @click="certInput?.click()">
        <AppIcon name="file" :size="18" /><span class="truncate">{{ cert ? cert[0] : t("corp.certPick") }}</span>
      </button>
      <input
        v-if="cert"
        v-model="certPassword"
        class="field"
        type="password"
        :placeholder="t('corp.certPassword')"
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
          :placeholder="t('corp.password')"
          :aria-label="t('corp.password')"
          autocomplete="current-password"
        />
      </div>
      <div class="flex gap-2">
        <button type="submit" class="btn btn-primary" :disabled="!gateway.trim() || saving">
          <AppIcon name="check" :size="18" />{{ t("common.add") }}
        </button>
        <button type="button" class="btn btn-quiet" @click="setupOpen = false">{{ t("common.cancel") }}</button>
      </div>
    </form>
  </div>
</template>
