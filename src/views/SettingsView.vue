<script setup lang="ts">
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { computed, onMounted, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon from "../components/AppIcon.vue";
import PerAppDialog from "../components/PerAppDialog.vue";
import ToggleSwitch from "../components/ToggleSwitch.vue";
import { LOCALES } from "../i18n";
import { api, type Mode, type PerAppMode, type PrivilegeState, type Settings } from "../lib/api";
import { useAppStore } from "../stores/app";

const store = useAppStore();
const { t } = useI18n();

const LOG_LEVELS = ["debug", "info", "warning", "error", "none"];
const LOCALE_NAMES: Record<string, string> = { ru: "Русский", en: "English" };
const MODES: Mode[] = ["proxy", "tun"];
const PER_APP_MODES: { value: PerAppMode; label: string }[] = [
  { value: "off", label: "settings.perAppOff" },
  { value: "allow", label: "settings.perAppAllow" },
  { value: "disallow", label: "settings.perAppDisallow" },
];

const choosingApps = ref(false);

function savePerAppMode(mode: string) {
  return save({ perApp: { mode: mode as PerAppMode, packages: store.settings!.perApp.packages } });
}

// Text fields are edited locally and saved when the user leaves them, so a
// half-typed port or address never restarts the connection.
const draft = reactive({ mixedPort: "", remoteDns: "", directDns: "", interfaceName: "", proxyUser: "", proxyPassword: "" });
watch(
  () => store.settings,
  (settings) => {
    if (!settings) return;
    draft.mixedPort = String(settings.mixedPort);
    draft.remoteDns = settings.remoteDns;
    draft.directDns = settings.directDns;
    draft.interfaceName = settings.interfaceName;
    draft.proxyUser = settings.proxyUser;
    draft.proxyPassword = settings.proxyPassword;
  },
  { immediate: true },
);

function saveCredential(key: "proxyUser" | "proxyPassword") {
  // SOCKS5 limits both to 255 bytes; empty values are not accepted.
  const value = draft[key].trim().slice(0, 255);
  if (value && value !== store.settings?.[key]) {
    save({ [key]: value });
  } else {
    draft[key] = store.settings?.[key] ?? "";
  }
}

async function copyProxyAddress() {
  const settings = store.settings!;
  const credentials = settings.proxyAuth
    ? `${encodeURIComponent(settings.proxyUser)}:${encodeURIComponent(settings.proxyPassword)}@`
    : "";
  await writeText(`socks5://${credentials}127.0.0.1:${settings.mixedPort}`);
  store.toast(t("common.copied"));
}

function saveInterfaceName() {
  // Linux interface names are limited to 15 characters; keep to that on every
  // platform for safety.
  const value = draft.interfaceName.trim().replace(/\s+/g, "").slice(0, 15);
  if (value && value !== store.settings?.interfaceName) {
    save({ interfaceName: value });
  } else {
    draft.interfaceName = store.settings?.interfaceName ?? "";
  }
}

function save(patch: Partial<Settings>) {
  return store.saveSettings(patch);
}

function savePort() {
  const port = Number(draft.mixedPort);
  if (Number.isInteger(port) && port >= 1 && port <= 65535) {
    if (port !== store.settings?.mixedPort) save({ mixedPort: port });
  } else {
    draft.mixedPort = String(store.settings?.mixedPort ?? "");
  }
}

function saveDns(key: "remoteDns" | "directDns") {
  const value = draft[key].trim();
  if (value && value !== store.settings?.[key]) {
    save({ [key]: value });
  } else {
    draft[key] = store.settings?.[key] ?? "";
  }
}

async function copyHwid() {
  await writeText(store.snapshot!.device.hwid);
  store.toast(t("common.copied"));
}

// Stealth (Android, needs root). The privilege level is detected without
// prompting; enabling stealth later is what triggers the superuser grant.
const privilege = ref<PrivilegeState | null>(null);
const PRIVILEGE_LABELS: Record<string, string> = {
  none: "settings.privNone",
  shizukuShell: "settings.privShizukuShell",
  shizukuRoot: "settings.privShizukuRoot",
  root: "settings.privRoot",
};
const privilegeLabel = computed(() => PRIVILEGE_LABELS[privilege.value?.level ?? "none"]);
const canStealth = computed(
  () => privilege.value?.level === "root" || privilege.value?.level === "shizukuRoot",
);

async function refreshPrivilege() {
  if (store.isDesktop) return;
  privilege.value = await api.privilegeState().catch(() => null);
}

async function authorizeShizuku() {
  const next = await store.run(() => api.requestShizuku());
  if (next) privilege.value = next;
}

onMounted(refreshPrivilege);
</script>

<template>
  <div v-if="store.settings && store.snapshot" class="px-4 py-5">
    <h1 class="mb-4 text-xl font-semibold">{{ t("settings.title") }}</h1>

    <div class="section-title">{{ t("settings.connection") }}</div>
    <div class="card mb-6 divide-y divide-line">
      <div v-if="store.tunSupported" class="px-4 py-3" role="radiogroup" :aria-label="t('settings.mode')">
        <div class="mb-2 font-medium">{{ t("settings.mode") }}</div>
        <button
          v-for="mode in MODES"
          :key="mode"
          role="radio"
          :aria-checked="store.settings.mode === mode"
          class="flex w-full items-start gap-3 rounded-xl px-2 py-2 text-left transition-colors hover:bg-raised"
          @click="save({ mode })"
        >
          <span
            class="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border-2 transition-colors duration-200"
            :class="store.settings.mode === mode ? 'border-accent bg-accent text-accent-ink' : 'border-line'"
          >
            <Transition name="pop">
              <AppIcon v-if="store.settings.mode === mode" name="check" :size="12" />
            </Transition>
          </span>
          <span>
            <span class="block text-sm font-medium">{{ t(mode === "proxy" ? "home.modeProxy" : "home.modeTun") }}</span>
            <span class="block text-xs text-muted">
              {{ t(mode === "proxy" ? "settings.modeProxyHint" : "settings.modeTunHint") }}
            </span>
          </span>
        </button>
      </div>
      <label v-if="store.isDesktop" class="flex items-center gap-3 px-4 py-3">
        <span class="flex-1 font-medium">{{ t("settings.mixedPort") }}</span>
        <input
          v-model="draft.mixedPort"
          class="field selectable w-24 text-right font-mono"
          inputmode="numeric"
          @blur="savePort"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
      </label>
      <div v-if="store.isDesktop" class="px-4 py-3">
        <div class="flex items-center gap-3">
          <div class="flex-1">
            <div class="font-medium">{{ t("settings.proxyAuth") }}</div>
            <div class="text-xs text-muted">{{ t("settings.proxyAuthHint") }}</div>
          </div>
          <ToggleSwitch
            :model-value="store.settings.proxyAuth"
            :label="t('settings.proxyAuth')"
            @update:model-value="(proxyAuth: boolean) => save({ proxyAuth })"
          />
        </div>
        <Transition name="pop">
          <div v-if="store.settings.proxyAuth" class="mt-3 grid grid-cols-2 gap-2">
            <label class="block">
              <span class="mb-1 block text-xs text-muted">{{ t("settings.proxyUser") }}</span>
              <input
                v-model="draft.proxyUser"
                class="field selectable font-mono text-xs"
                spellcheck="false"
                autocapitalize="off"
                autocomplete="off"
                @blur="saveCredential('proxyUser')"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </label>
            <label class="block">
              <span class="mb-1 block text-xs text-muted">{{ t("settings.proxyPassword") }}</span>
              <input
                v-model="draft.proxyPassword"
                class="field selectable font-mono text-xs"
                spellcheck="false"
                autocapitalize="off"
                autocomplete="off"
                @blur="saveCredential('proxyPassword')"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </label>
          </div>
        </Transition>
        <button class="btn btn-quiet mt-3 w-full" @click="copyProxyAddress">
          <AppIcon name="copy" :size="16" />{{ t("settings.proxyCopy") }}
        </button>
      </div>
      <div v-if="store.isDesktop" class="flex items-center gap-3 px-4 py-3">
        <div class="flex-1">
          <div class="font-medium">{{ t("settings.allowLan") }}</div>
          <div class="text-xs text-muted">{{ t("settings.allowLanHint") }}</div>
        </div>
        <ToggleSwitch
          :model-value="store.settings.allowLan"
          :label="t('settings.allowLan')"
          @update:model-value="(allowLan: boolean) => save({ allowLan })"
        />
      </div>
      <div class="flex items-center gap-3 px-4 py-3">
        <div class="flex-1">
          <div class="font-medium">{{ t("settings.ipv6") }}</div>
          <div class="text-xs text-muted">{{ t("settings.ipv6Hint") }}</div>
        </div>
        <ToggleSwitch
          :model-value="store.settings.ipv6"
          :label="t('settings.ipv6')"
          @update:model-value="(ipv6: boolean) => save({ ipv6 })"
        />
      </div>
      <label class="block px-4 py-3">
        <span class="mb-1.5 block font-medium">{{ t("settings.interfaceName") }}</span>
        <input
          v-model="draft.interfaceName"
          class="field selectable font-mono text-xs"
          spellcheck="false"
          autocapitalize="off"
          maxlength="15"
          @blur="saveInterfaceName"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <span class="mt-1.5 block text-xs text-muted">
          {{ store.isDesktop ? t("settings.interfaceNameHintDesktop") : t("settings.interfaceNameHintMobile") }}
        </span>
      </label>
    </div>

    <template v-if="!store.isDesktop">
      <div class="section-title">{{ t("settings.perApp") }}</div>
      <div class="card mb-6 divide-y divide-line">
        <label class="flex items-center gap-3 px-4 py-3">
          <span class="flex-1 font-medium">{{ t("settings.perAppMode") }}</span>
          <select
            class="field w-48 px-2"
            :value="store.settings.perApp.mode"
            @change="savePerAppMode(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="item in PER_APP_MODES" :key="item.value" :value="item.value">{{ t(item.label) }}</option>
          </select>
        </label>
        <Transition name="pop">
          <button
            v-if="store.settings.perApp.mode !== 'off'"
            class="flex w-full items-center gap-3 px-4 py-3 text-left"
            @click="choosingApps = true"
          >
            <span class="flex-1">
              <span class="block font-medium">{{ t("settings.perAppChoose") }}</span>
              <span class="block text-xs text-muted">
                {{ t("settings.perAppSelected", { count: store.settings.perApp.packages.length }) }}
              </span>
            </span>
            <AppIcon name="chevron" class="text-muted" />
          </button>
        </Transition>
      </div>

      <div class="section-title">{{ t("settings.stealth") }}</div>
      <div class="card mb-6 divide-y divide-line">
        <div class="flex items-center gap-3 px-4 py-3">
          <div class="min-w-0 flex-1">
            <div class="font-medium">{{ t("settings.privilege") }}</div>
            <div class="text-xs text-muted">{{ t(privilegeLabel) }}</div>
          </div>
          <button
            v-if="privilege?.shizukuAvailable && !privilege?.shizukuGranted"
            class="btn btn-quiet"
            @click="authorizeShizuku"
          >
            {{ t("settings.privAuthorizeShizuku") }}
          </button>
          <button v-else class="icon-btn" :aria-label="t('common.refresh')" :title="t('common.refresh')" @click="refreshPrivilege">
            <AppIcon name="refresh" :size="18" />
          </button>
        </div>
        <div class="flex items-center gap-3 px-4 py-3">
          <div class="flex-1">
            <div class="font-medium">{{ t("settings.stealthEnable") }}</div>
            <div class="text-xs text-muted">{{ canStealth ? t("settings.stealthHint") : t("settings.stealthNeedsRoot") }}</div>
          </div>
          <ToggleSwitch
            :model-value="store.settings.stealth"
            :disabled="!canStealth"
            :label="t('settings.stealthEnable')"
            @update:model-value="(stealth: boolean) => save({ stealth })"
          />
        </div>
        <Transition name="pop">
          <div v-if="store.settings.stealth && canStealth" class="flex items-center gap-3 px-4 py-3">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.stealthRootTun") }}</div>
              <div class="text-xs text-muted">{{ t("settings.stealthRootTunHint") }}</div>
            </div>
            <ToggleSwitch
              :model-value="store.settings.stealthRootTun"
              :label="t('settings.stealthRootTun')"
              @update:model-value="(stealthRootTun: boolean) => save({ stealthRootTun })"
            />
          </div>
        </Transition>
      </div>
    </template>

    <div class="section-title">{{ t("settings.dns") }}</div>
    <div class="card mb-6 divide-y divide-line">
      <label class="block px-4 py-3">
        <span class="mb-1.5 block font-medium">{{ t("settings.remoteDns") }}</span>
        <input
          v-model="draft.remoteDns"
          class="field selectable font-mono text-xs"
          spellcheck="false"
          autocapitalize="off"
          @blur="saveDns('remoteDns')"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
      </label>
      <label class="block px-4 py-3">
        <span class="mb-1.5 block font-medium">{{ t("settings.directDns") }}</span>
        <input
          v-model="draft.directDns"
          class="field selectable font-mono text-xs"
          spellcheck="false"
          autocapitalize="off"
          @blur="saveDns('directDns')"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <span class="mt-1.5 block text-xs text-muted">{{ t("settings.directDnsHint") }}</span>
      </label>
    </div>

    <div class="section-title">{{ t("settings.subscriptions") }}</div>
    <div class="card mb-6 divide-y divide-line">
      <div class="flex items-center gap-3 px-4 py-3">
        <div class="flex-1">
          <div class="font-medium">{{ t("settings.sendHwid") }}</div>
          <div class="text-xs text-muted">{{ t("settings.sendHwidHint") }}</div>
        </div>
        <ToggleSwitch
          :model-value="store.settings.sendHwid"
          :label="t('settings.sendHwid')"
          @update:model-value="(sendHwid: boolean) => save({ sendHwid })"
        />
      </div>
      <div class="flex items-center gap-2 py-2 pr-2 pl-4">
        <div class="min-w-0 flex-1">
          <div class="font-medium">{{ t("settings.hwid") }}</div>
          <div class="selectable truncate font-mono text-xs text-muted">{{ store.snapshot.device.hwid }}</div>
        </div>
        <button class="icon-btn" :aria-label="t('common.copy')" :title="t('common.copy')" @click="copyHwid">
          <AppIcon name="copy" :size="18" />
        </button>
      </div>
    </div>

    <div class="section-title">{{ t("settings.app") }}</div>
    <div class="card mb-6 divide-y divide-line">
      <label class="flex items-center gap-3 px-4 py-3">
        <span class="flex-1 font-medium">{{ t("settings.language") }}</span>
        <select
          class="field w-44 px-2"
          :value="store.settings.locale ?? ''"
          @change="save({ locale: ($event.target as HTMLSelectElement).value || null })"
        >
          <option value="">{{ t("settings.languageSystem") }}</option>
          <option v-for="locale in LOCALES" :key="locale" :value="locale">{{ LOCALE_NAMES[locale] }}</option>
        </select>
      </label>
      <label class="flex items-center gap-3 px-4 py-3">
        <span class="flex-1 font-medium">{{ t("settings.logLevel") }}</span>
        <select
          class="field w-44 px-2"
          :value="store.settings.logLevel"
          @change="save({ logLevel: ($event.target as HTMLSelectElement).value })"
        >
          <option v-for="level in LOG_LEVELS" :key="level" :value="level">{{ level }}</option>
        </select>
      </label>
      <div class="flex items-center gap-3 px-4 py-3">
        <span class="flex-1 font-medium">{{ t("settings.version") }}</span>
        <span class="selectable font-mono text-sm text-muted">{{ store.snapshot.version }}</span>
      </div>
    </div>

    <button v-if="store.isDesktop" class="btn btn-danger w-full" @click="api.quit()">
      <AppIcon name="power" :size="18" />{{ t("settings.quit") }}
    </button>
    <Transition name="sheet">
      <PerAppDialog v-if="choosingApps" @close="choosingApps = false" />
    </Transition>
  </div>
</template>
