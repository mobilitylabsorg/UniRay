<script setup lang="ts">
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AppIcon, { type IconName } from "../components/AppIcon.vue";
import BackupModal from "../components/BackupModal.vue";
import DiagnosticsModal from "../components/DiagnosticsModal.vue";
import PerAppDialog from "../components/PerAppDialog.vue";
import SettingRow from "../components/SettingRow.vue";
import ServiceCheckModal from "../components/ServiceCheckModal.vue";
import SpeedTestModal from "../components/SpeedTestModal.vue";
import ToggleSwitch from "../components/ToggleSwitch.vue";
import UpdateCard from "../components/UpdateCard.vue";
import { LOCALES } from "../i18n";
import { api, type DelayTestMode, type Mode, type PerAppMode, type PrivilegeState, type Settings } from "../lib/api";
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
const DELAY_TEST_MODES: { value: DelayTestMode; label: string }[] = [
  { value: "http", label: "settings.delayTestModeHttp" },
  { value: "tcp", label: "settings.delayTestModeTcp" },
];
const TLS_FINGERPRINTS = [
  { value: "auto", label: "settings.fpAuto" },
  { value: "chrome", label: "settings.fpChrome" },
  { value: "firefox", label: "settings.fpFirefox" },
  { value: "safari", label: "settings.fpSafari" },
  { value: "ios", label: "settings.fpIos" },
  { value: "edge", label: "settings.fpEdge" },
  { value: "random", label: "settings.fpRandom" },
  { value: "randomized", label: "settings.fpRandomized" },
];
const WG_MTU_OPTIONS = [1280, 1380, 1420, 1500];
const UA_PRESETS = [
  { value: "", label: "settings.uaUniRay" },
  { value: "happ", label: "settings.uaHappAndroid" },
  { value: "incy", label: "settings.uaIncy" },
  { value: "v2rayng", label: "settings.uaV2rayng" },
  { value: "nekobox", label: "settings.uaNekobox" },
  { value: "karing", label: "settings.uaKaring" },
  { value: "hiddify", label: "settings.uaHiddify" },
  { value: "sing-box", label: "settings.uaSingBox" },
];

const choosingApps = ref(false);
const showBackup = ref(false);
const showServiceCheck = ref(false);
const showSpeedTest = ref(false);
const showDiagnostics = ref(false);

const TOOLS: { key: string; icon: IconName; open: () => void }[] = [
  { key: "serviceCheck", icon: "check", open: () => (showServiceCheck.value = true) },
  { key: "speedTest", icon: "gauge", open: () => (showSpeedTest.value = true) },
  { key: "diagnostics", icon: "alert", open: () => (showDiagnostics.value = true) },
  { key: "backup", icon: "copy", open: () => (showBackup.value = true) },
];

function savePerAppMode(mode: string) {
  return save({ perApp: { mode: mode as PerAppMode, packages: store.settings!.perApp.packages } });
}

// Text fields are edited locally and saved when the user leaves them, so a
// half-typed port or address never restarts the connection.
const draft = reactive({
  mixedPort: "",
  remoteDns: "",
  directDns: "",
  interfaceName: "",
  proxyUser: "",
  proxyPassword: "",
  probeUrl: "",
  tlsFragmentLength: "",
  tlsFragmentInterval: "",
  tlsFragmentPackets: "",
});
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
    draft.probeUrl = settings.probeUrl;
    draft.tlsFragmentLength = settings.tlsFragmentLength;
    draft.tlsFragmentInterval = settings.tlsFragmentInterval;
    draft.tlsFragmentPackets = settings.tlsFragmentPackets;
  },
  { immediate: true },
);

function saveDraft(key: "probeUrl" | "tlsFragmentLength" | "tlsFragmentInterval" | "tlsFragmentPackets") {
  const value = draft[key].trim();
  if (value !== store.settings?.[key]) {
    save({ [key]: value });
  } else {
    draft[key] = store.settings?.[key] ?? "";
  }
}

function setProbeUrlPreset(url: string) {
  draft.probeUrl = url;
  save({ probeUrl: url });
}

function setFragmentPreset(length: string, interval: string) {
  draft.tlsFragmentLength = length;
  draft.tlsFragmentInterval = interval;
  save({ tlsFragmentLength: length, tlsFragmentInterval: interval });
}

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

// Saved like the other settings (it takes effect on the next connection),
// and applied to the running tunnel at once. Turning it on also brings up
// Android's battery optimization dialog: without the exemption Doze still
// cuts the app's network off with the screen off.
const batteryExempt = ref<boolean | null>(null);
async function saveKeepAwake(keepAwake: boolean) {
  await save({ keepAwake });
  try {
    batteryExempt.value = (await api.setKeepAwake(keepAwake)).batteryExempt;
  } catch {
    batteryExempt.value = null;
  }
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
// A hint, not a lock: a kernel can list TPROXY and still handle it badly
// (some custom ROMs), and the root TUN is the way out then.
const stealthRootTunHint = computed(() => {
  if (privilege.value?.hasTproxy === true) return "settings.stealthRootTunTproxySupported";
  if (privilege.value?.hasTproxy === false) return "settings.stealthRootTunTproxyMissing";
  return "settings.stealthRootTunHint";
});

async function refreshPrivilege() {
  if (store.isDesktop) return;
  privilege.value = await api.privilegeState().catch(() => null);
}

async function authorizeShizuku() {
  const next = await store.run(() => api.requestShizuku());
  if (next) privilege.value = next;
}

// The Google Play edition has no stealth mode: nothing to ask about.
onMounted(() => {
  if (store.snapshot?.features.stealth) void refreshPrivilege();
});

// The page's sections, in order: the index on a wide window lists them, and
// the one at the top of the view is marked there.
const sections = computed(() => {
  const desktop = store.isDesktop;
  const list: { id: string; title: string }[] = [{ id: "connection", title: t("settings.connection") }];
  if (store.tunSupported) list.push({ id: "mode", title: t("settings.mode") });
  if (desktop) list.push({ id: "proxy", title: t("settings.sectionProxy") });
  list.push({ id: "network", title: t("settings.network") }, { id: "dns", title: t("settings.dns") });
  if (!desktop) list.push({ id: "perApp", title: t("settings.perApp") });
  if (!desktop && store.snapshot?.features.stealth) list.push({ id: "stealth", title: t("settings.stealth") });
  list.push(
    { id: "subscriptions", title: t("settings.subscriptions") },
    { id: "delay", title: t("settings.delayTesting") },
    { id: "dpi", title: t("settings.dpiBypass") },
    { id: "advanced", title: t("settings.advanced") },
    { id: "tools", title: t("tools.title") },
    { id: "app", title: t("settings.app") },
  );
  return list;
});

const current = ref("connection");
let observer: IntersectionObserver | null = null;

function jump(id: string) {
  current.value = id;
  document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
}

onMounted(() => {
  // A section counts as current while it crosses the upper third of the view.
  observer = new IntersectionObserver(
    (entries) => {
      const seen = entries.filter((entry) => entry.isIntersecting).map((entry) => entry.target.id.replace("settings-", ""));
      if (seen.length) current.value = seen[0];
    },
    { root: document.querySelector("main"), rootMargin: "0px 0px -66% 0px" },
  );
  for (const element of document.querySelectorAll("[id^='settings-']")) observer.observe(element);
});
onBeforeUnmount(() => observer?.disconnect());

const proxyAddress = computed(() => `socks5://127.0.0.1:${store.settings?.mixedPort ?? ""}`);
</script>

<template>
  <div v-if="store.settings && store.snapshot" class="page lg:max-w-6xl 2xl:max-w-[96rem]">
    <header class="page-header">
      <h1 class="page-title">{{ t("settings.title") }}</h1>
    </header>

    <div class="lg:flex lg:items-start lg:gap-10">
      <!-- A wide window: the sections listed on the left, the current one marked. -->
      <nav v-if="!store.isTv" class="sticky top-6 hidden w-48 shrink-0 flex-col gap-0.5 lg:flex" :aria-label="t('settings.sections')">
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          class="rounded-lg px-3 py-1.5 text-left text-sm leading-snug transition-colors"
          :class="current === section.id ? 'bg-raised font-medium text-ink' : 'text-muted hover:text-ink'"
          :aria-current="current === section.id || undefined"
          @click="jump(section.id)"
        >
          {{ section.title }}
        </button>
      </nav>

      <div class="min-w-0 flex-1 lg:max-w-3xl 2xl:max-w-none 2xl:columns-2 2xl:gap-8">
        <section id="settings-connection" class="settings-section">
          <h2 class="section-title">{{ t("settings.connection") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.autoConnect')" :hint="t('settings.autoConnectHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.autoConnect"
                :label="t('settings.autoConnect')"
                @update:model-value="(autoConnect: boolean) => save({ autoConnect })"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.autoFailover')" :hint="t('settings.autoFailoverHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.autoFailover"
                :label="t('settings.autoFailover')"
                @update:model-value="(autoFailover: boolean) => save({ autoFailover })"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.autoRecover')" :hint="t('settings.autoRecoverHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.autoRecover"
                :label="t('settings.autoRecover')"
                @update:model-value="(autoRecover: boolean) => save({ autoRecover })"
              />
            </SettingRow>
            <SettingRow
              v-if="!store.isDesktop"
              :label="t('settings.keepAwake')"
              :hint="t(store.isTv ? 'settings.keepAwakeTvHint' : 'settings.keepAwakeHint')"
            >
              <template #default="{ id }">
                <ToggleSwitch :id="id" :model-value="store.settings.keepAwake" :label="t('settings.keepAwake')" @update:model-value="saveKeepAwake" />
              </template>
              <template #below>
                <p v-if="store.settings.keepAwake && batteryExempt === false" class="mt-2 rounded-lg bg-warn-soft px-3 py-2 text-xs text-warn">
                  {{ t("settings.keepAwakeNoExemption") }}
                </p>
              </template>
            </SettingRow>
          </div>
        </section>

        <section v-if="store.tunSupported" id="settings-mode" class="settings-section">
          <h2 class="section-title">{{ t("settings.mode") }}</h2>
          <div class="card divide-y divide-line overflow-hidden" role="radiogroup" :aria-label="t('settings.mode')">
            <button
              v-for="mode in MODES"
              :key="mode"
              role="radio"
              :aria-checked="store.settings.mode === mode"
              class="row-button"
              @click="save({ mode })"
            >
              <span class="radio-mark" :class="{ 'is-on': store.settings.mode === mode }">
                <Transition name="pop">
                  <AppIcon v-if="store.settings.mode === mode" name="check" :size="12" />
                </Transition>
              </span>
              <span class="min-w-0">
                <span class="row-label">{{ t(mode === "proxy" ? "home.modeProxy" : "home.modeTun") }}</span>
                <span class="row-hint">{{ t(mode === "proxy" ? "settings.modeProxyHint" : "settings.modeTunHint") }}</span>
              </span>
            </button>
          </div>
        </section>

        <section v-if="store.isDesktop" id="settings-proxy" class="settings-section">
          <h2 class="section-title">{{ t("settings.sectionProxy") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.mixedPort')">
              <input
                :id="id"
                v-model="draft.mixedPort"
                class="field selectable w-24 text-right font-mono"
                inputmode="numeric"
                @blur="savePort"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.proxyAddress')" :hint="proxyAddress">
              <button :id="id" class="icon-btn" :aria-label="t('settings.proxyCopy')" :title="t('settings.proxyCopy')" @click="copyProxyAddress">
                <AppIcon name="copy" :size="18" />
              </button>
            </SettingRow>
            <SettingRow :label="t('settings.proxyAuth')" :hint="t('settings.proxyAuthHint')">
              <template #default="{ id }">
                <ToggleSwitch
                  :id="id"
                  :model-value="store.settings.proxyAuth"
                  :label="t('settings.proxyAuth')"
                  @update:model-value="(proxyAuth: boolean) => save({ proxyAuth })"
                />
              </template>
              <template #below>
                <Transition name="pop">
                  <div v-if="store.settings.proxyAuth" class="mt-3 grid grid-cols-2 gap-2">
                    <input
                      v-model="draft.proxyUser"
                      class="field selectable font-mono text-xs"
                      :placeholder="t('settings.proxyUser')"
                      :aria-label="t('settings.proxyUser')"
                      spellcheck="false"
                      autocapitalize="off"
                      autocomplete="off"
                      @blur="saveCredential('proxyUser')"
                      @keydown.enter="($event.target as HTMLInputElement).blur()"
                    />
                    <input
                      v-model="draft.proxyPassword"
                      class="field selectable font-mono text-xs"
                      :placeholder="t('settings.proxyPassword')"
                      :aria-label="t('settings.proxyPassword')"
                      spellcheck="false"
                      autocapitalize="off"
                      autocomplete="off"
                      @blur="saveCredential('proxyPassword')"
                      @keydown.enter="($event.target as HTMLInputElement).blur()"
                    />
                  </div>
                </Transition>
              </template>
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.allowLan')" :hint="t('settings.allowLanHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.allowLan"
                :label="t('settings.allowLan')"
                @update:model-value="(allowLan: boolean) => save({ allowLan })"
              />
            </SettingRow>
          </div>
        </section>

        <section id="settings-network" class="settings-section">
          <h2 class="section-title">{{ t("settings.network") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.ipv6')" :hint="t('settings.ipv6Hint')">
              <ToggleSwitch :id="id" :model-value="store.settings.ipv6" :label="t('settings.ipv6')" @update:model-value="(ipv6: boolean) => save({ ipv6 })" />
            </SettingRow>
            <SettingRow v-if="store.snapshot.features.adBlock" v-slot="{ id }" :label="t('settings.blockAds')" :hint="t('settings.blockAdsHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.blockAds"
                :label="t('settings.blockAds')"
                @update:model-value="(blockAds: boolean) => save({ blockAds })"
              />
            </SettingRow>
          </div>
        </section>

        <section id="settings-dns" class="settings-section">
          <h2 class="section-title">{{ t("settings.dns") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.remoteDns')" wide>
              <input
                :id="id"
                v-model="draft.remoteDns"
                class="field selectable font-mono text-xs"
                spellcheck="false"
                autocapitalize="off"
                @blur="saveDns('remoteDns')"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.directDns')" :hint="t('settings.directDnsHint')" wide>
              <input
                :id="id"
                v-model="draft.directDns"
                class="field selectable font-mono text-xs transition-opacity disabled:opacity-50"
                :disabled="store.settings.directDnsFromNetwork"
                spellcheck="false"
                autocapitalize="off"
                @blur="saveDns('directDns')"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.directDnsFromNetwork')" :hint="t('settings.directDnsFromNetworkHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.directDnsFromNetwork"
                :label="t('settings.directDnsFromNetwork')"
                @update:model-value="(directDnsFromNetwork: boolean) => save({ directDnsFromNetwork })"
              />
            </SettingRow>
          </div>
        </section>

        <section v-if="!store.isDesktop" id="settings-perApp" class="settings-section">
          <h2 class="section-title">{{ t("settings.perApp") }}</h2>
          <div class="card divide-y divide-line overflow-hidden">
            <SettingRow v-slot="{ id }" :label="t('settings.perAppMode')">
              <select
                :id="id"
                class="field w-44 px-2 sm:w-52"
                :value="store.settings.perApp.mode"
                @change="savePerAppMode(($event.target as HTMLSelectElement).value)"
              >
                <option v-for="item in PER_APP_MODES" :key="item.value" :value="item.value">{{ t(item.label) }}</option>
              </select>
            </SettingRow>
            <button v-if="store.settings.perApp.mode !== 'off'" class="row-button" @click="choosingApps = true">
              <span class="min-w-0 flex-1">
                <span class="row-label">{{ t("settings.perAppChoose") }}</span>
                <span class="row-hint">{{ t("settings.perAppSelected", { count: store.settings.perApp.packages.length }) }}</span>
              </span>
              <AppIcon name="chevron" :size="18" class="shrink-0 text-muted" />
            </button>
            <p v-if="store.settings.stealth && canStealth" class="bg-warn-soft px-4 py-2.5 text-xs text-warn">
              {{ t("settings.perAppStealthWarning") }}
            </p>
          </div>
        </section>

        <section v-if="!store.isDesktop && store.snapshot.features.stealth" id="settings-stealth" class="settings-section">
          <h2 class="section-title">{{ t("settings.stealth") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow :label="t('settings.privilege')" :hint="t(privilegeLabel)">
              <button v-if="privilege?.shizukuAvailable && !privilege?.shizukuGranted" class="btn btn-quiet btn-sm" @click="authorizeShizuku">
                {{ t("settings.privAuthorizeShizuku") }}
              </button>
              <button v-else class="icon-btn" :aria-label="t('common.refresh')" :title="t('common.refresh')" @click="refreshPrivilege">
                <AppIcon name="refresh" :size="18" />
              </button>
            </SettingRow>
            <SettingRow
              v-slot="{ id }"
              :label="t('settings.stealthEnable')"
              :hint="canStealth ? t('settings.stealthHint') : t('settings.stealthNeedsRoot')"
            >
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.stealth"
                :disabled="!canStealth"
                :label="t('settings.stealthEnable')"
                @update:model-value="(stealth: boolean) => save({ stealth })"
              />
            </SettingRow>
            <SettingRow
              v-if="store.settings.stealth && canStealth"
              v-slot="{ id }"
              :label="t('settings.stealthRootTun')"
              :hint="t(stealthRootTunHint)"
            >
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.stealthRootTun"
                :label="t('settings.stealthRootTun')"
                @update:model-value="(stealthRootTun: boolean) => save({ stealthRootTun })"
              />
            </SettingRow>
          </div>
        </section>

        <section id="settings-subscriptions" class="settings-section">
          <h2 class="section-title">{{ t("settings.subscriptions") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.sendHwid')" :hint="t('settings.sendHwidHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.sendHwid"
                :label="t('settings.sendHwid')"
                @update:model-value="(sendHwid: boolean) => save({ sendHwid })"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.hwid')" :hint="store.snapshot.device.hwid">
              <button :id="id" class="icon-btn" :aria-label="t('common.copy')" :title="t('common.copy')" @click="copyHwid">
                <AppIcon name="copy" :size="18" />
              </button>
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.autoUpdate')" :hint="t('settings.autoUpdateHint')" wide>
              <select
                :id="id"
                class="field px-2"
                :value="store.settings.autoUpdateHours"
                @change="save({ autoUpdateHours: Number(($event.target as HTMLSelectElement).value) })"
              >
                <option :value="0">{{ t("settings.autoUpdateDisabled") }}</option>
                <option :value="6">{{ t("settings.autoUpdate6h") }}</option>
                <option :value="12">{{ t("settings.autoUpdate12h") }}</option>
                <option :value="24">{{ t("settings.autoUpdate24h") }}</option>
              </select>
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.userAgent')" :hint="t('settings.userAgentHint')" wide>
              <select
                :id="id"
                class="field px-2"
                :value="store.settings.userAgent || ''"
                @change="save({ userAgent: ($event.target as HTMLSelectElement).value })"
              >
                <option v-for="ua in UA_PRESETS" :key="ua.value" :value="ua.value">{{ t(ua.label) }}</option>
              </select>
            </SettingRow>
          </div>
        </section>

        <section id="settings-delay" class="settings-section">
          <h2 class="section-title">{{ t("settings.delayTesting") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow v-slot="{ id }" :label="t('settings.delayTestMode')" :hint="t('settings.delayTestModeHint')" wide>
              <select
                :id="id"
                class="field px-2"
                :value="store.settings.delayTestMode"
                @change="save({ delayTestMode: ($event.target as HTMLSelectElement).value as DelayTestMode })"
              >
                <option v-for="item in DELAY_TEST_MODES" :key="item.value" :value="item.value">{{ t(item.label) }}</option>
              </select>
            </SettingRow>
            <SettingRow v-if="store.settings.delayTestMode === 'http'" :label="t('settings.probeUrl')" :hint="t('settings.probeUrlHint')" wide>
              <template #default="{ id }">
                <input
                  :id="id"
                  v-model="draft.probeUrl"
                  class="field selectable font-mono text-xs"
                  spellcheck="false"
                  autocapitalize="off"
                  @blur="saveDraft('probeUrl')"
                  @keydown.enter="($event.target as HTMLInputElement).blur()"
                />
              </template>
              <template #below>
                <div class="mt-2 flex flex-wrap justify-end gap-2">
                  <button type="button" class="btn btn-quiet btn-sm" @click="setProbeUrlPreset('http://cp.cloudflare.com/generate_204')">
                    Cloudflare (HTTP)
                  </button>
                  <button type="button" class="btn btn-quiet btn-sm" @click="setProbeUrlPreset('https://www.gstatic.com/generate_204')">
                    Google (HTTPS)
                  </button>
                </div>
              </template>
            </SettingRow>
          </div>
        </section>

        <section id="settings-dpi" class="settings-section">
          <h2 class="section-title">{{ t("settings.dpiBypass") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow :label="t('settings.tlsFragment')" :hint="t('settings.tlsFragmentHint')">
              <template #default="{ id }">
                <ToggleSwitch
                  :id="id"
                  :model-value="store.settings.tlsFragment"
                  :label="t('settings.tlsFragment')"
                  @update:model-value="(tlsFragment: boolean) => save({ tlsFragment })"
                />
              </template>
              <template #below>
                <Transition name="pop">
                  <div v-if="store.settings.tlsFragment" class="mt-3 rounded-xl bg-raised/60 p-3">
                    <div class="mb-3 flex flex-wrap items-center gap-2">
                      <span class="text-xs text-muted">{{ t("settings.presets") }}</span>
                      <button type="button" class="btn btn-quiet btn-sm bg-surface" @click="setFragmentPreset('100-200', '10-20')">
                        {{ t("settings.presetStandard") }}
                      </button>
                      <button type="button" class="btn btn-quiet btn-sm bg-surface" @click="setFragmentPreset('10-30', '5-15')">
                        {{ t("settings.presetAggressive") }}
                      </button>
                    </div>
                    <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
                      <label class="block">
                        <span class="mb-1 block text-xs text-muted">{{ t("settings.tlsFragmentPackets") }}</span>
                        <input
                          v-model="draft.tlsFragmentPackets"
                          class="field selectable font-mono text-xs"
                          spellcheck="false"
                          autocapitalize="off"
                          placeholder="tlshello"
                          @blur="saveDraft('tlsFragmentPackets')"
                          @keydown.enter="($event.target as HTMLInputElement).blur()"
                        />
                      </label>
                      <label class="block">
                        <span class="mb-1 block text-xs text-muted">{{ t("settings.tlsFragmentLength") }}</span>
                        <input
                          v-model="draft.tlsFragmentLength"
                          class="field selectable font-mono text-xs"
                          spellcheck="false"
                          autocapitalize="off"
                          placeholder="100-200"
                          @blur="saveDraft('tlsFragmentLength')"
                          @keydown.enter="($event.target as HTMLInputElement).blur()"
                        />
                      </label>
                      <label class="block">
                        <span class="mb-1 block text-xs text-muted">{{ t("settings.tlsFragmentInterval") }}</span>
                        <input
                          v-model="draft.tlsFragmentInterval"
                          class="field selectable font-mono text-xs"
                          spellcheck="false"
                          autocapitalize="off"
                          placeholder="10-20"
                          @blur="saveDraft('tlsFragmentInterval')"
                          @keydown.enter="($event.target as HTMLInputElement).blur()"
                        />
                      </label>
                    </div>
                  </div>
                </Transition>
              </template>
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.tlsFingerprint')" :hint="t('settings.tlsFingerprintHint')" wide>
              <select
                :id="id"
                class="field px-2"
                :value="store.settings.tlsFingerprint || 'auto'"
                @change="save({ tlsFingerprint: ($event.target as HTMLSelectElement).value })"
              >
                <option v-for="fp in TLS_FINGERPRINTS" :key="fp.value" :value="fp.value">{{ t(fp.label) }}</option>
              </select>
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.tcpFastOpen')" :hint="t('settings.tcpFastOpenHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.tcpFastOpen"
                :label="t('settings.tcpFastOpen')"
                @update:model-value="(tcpFastOpen: boolean) => save({ tcpFastOpen })"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.allowInsecure')" :hint="t('settings.allowInsecureHint')">
              <ToggleSwitch
                :id="id"
                :model-value="store.settings.allowInsecure"
                :label="t('settings.allowInsecure')"
                @update:model-value="(allowInsecure: boolean) => save({ allowInsecure })"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.wgMtu')" :hint="t('settings.wgMtuHint')" wide>
              <select
                :id="id"
                class="field px-2"
                :value="store.settings.wgMtu || 1280"
                @change="save({ wgMtu: Number(($event.target as HTMLSelectElement).value) })"
              >
                <option v-for="mtu in WG_MTU_OPTIONS" :key="mtu" :value="mtu">
                  {{ mtu }} {{ mtu === 1280 ? t("settings.wgMtuRecommended") : "" }}
                </option>
              </select>
            </SettingRow>
          </div>
        </section>

        <section id="settings-advanced" class="settings-section">
          <h2 class="section-title">{{ t("settings.advanced") }}</h2>
          <div class="card divide-y divide-line">
            <SettingRow
              v-slot="{ id }"
              :label="t('settings.interfaceName')"
              :hint="store.isDesktop ? t('settings.interfaceNameHintDesktop') : t('settings.interfaceNameHintMobile')"
            >
              <input
                :id="id"
                v-model="draft.interfaceName"
                class="field selectable w-32 font-mono text-xs"
                spellcheck="false"
                autocapitalize="off"
                maxlength="15"
                @blur="saveInterfaceName"
                @keydown.enter="($event.target as HTMLInputElement).blur()"
              />
            </SettingRow>
            <SettingRow v-slot="{ id }" :label="t('settings.logLevel')">
              <select :id="id" class="field w-32 px-2" :value="store.settings.logLevel" @change="save({ logLevel: ($event.target as HTMLSelectElement).value })">
                <option v-for="level in LOG_LEVELS" :key="level" :value="level">{{ level }}</option>
              </select>
            </SettingRow>
          </div>
        </section>

        <section id="settings-tools" class="settings-section">
          <h2 class="section-title">{{ t("tools.title") }}</h2>
          <div class="card divide-y divide-line overflow-hidden">
            <button v-for="tool in TOOLS" :key="tool.key" class="row-button" @click="tool.open()">
              <span class="flex size-9 shrink-0 items-center justify-center rounded-xl bg-raised text-muted">
                <AppIcon :name="tool.icon" :size="18" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="row-label">{{ t(`tools.${tool.key}`) }}</span>
                <span class="row-hint">{{ t(`tools.${tool.key}Hint`) }}</span>
              </span>
              <AppIcon name="chevron" :size="18" class="shrink-0 text-muted" />
            </button>
          </div>
        </section>

        <section id="settings-app" class="settings-section">
          <h2 class="section-title">{{ t("settings.app") }}</h2>
          <div class="card divide-y divide-line overflow-hidden">
            <SettingRow v-slot="{ id }" :label="t('settings.language')">
              <select
                :id="id"
                class="field w-40 px-2"
                :value="store.settings.locale ?? ''"
                @change="save({ locale: ($event.target as HTMLSelectElement).value || null })"
              >
                <option value="">{{ t("settings.languageSystem") }}</option>
                <option v-for="locale in LOCALES" :key="locale" :value="locale">{{ LOCALE_NAMES[locale] }}</option>
              </select>
            </SettingRow>
            <UpdateCard v-if="store.snapshot.features.updates" />
            <div v-else class="row">
              <span class="min-w-0 flex-1 font-medium">{{ t("settings.version") }}</span>
              <span class="selectable font-mono text-sm text-muted">{{ store.snapshot.version }}</span>
            </div>
            <button v-if="store.isDesktop" class="row-button text-danger" @click="api.quit()">
              <AppIcon name="power" :size="18" class="shrink-0" />
              <span class="font-medium">{{ t("settings.quit") }}</span>
            </button>
          </div>
        </section>
      </div>
    </div>

    <Transition name="sheet">
      <PerAppDialog v-if="choosingApps" @close="choosingApps = false" />
    </Transition>
    <Transition name="sheet">
      <ServiceCheckModal v-if="showServiceCheck" @close="showServiceCheck = false" />
    </Transition>
    <Transition name="sheet">
      <SpeedTestModal v-if="showSpeedTest" @close="showSpeedTest = false" />
    </Transition>
    <Transition name="sheet">
      <DiagnosticsModal v-if="showDiagnostics" @close="showDiagnostics = false" />
    </Transition>
    <Transition name="sheet">
      <BackupModal v-if="showBackup" @close="showBackup = false" />
    </Transition>
  </div>
</template>

<style scoped>
@reference "../styles.css";

/* A titled group; in two columns it never breaks across them. */
.settings-section {
  @apply mb-7 break-inside-avoid scroll-mt-6;
}
</style>
