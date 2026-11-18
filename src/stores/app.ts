import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";

import { i18n, resolveLocale } from "../i18n";
import {
  api,
  events,
  toCmdError,
  type CmdError,
  type LogLine,
  type Server,
  type Settings,
  type Snapshot,
  type Traffic,
} from "../lib/api";

const MAX_LOG_LINES = 2000;
/** Seconds of speed the traffic chart shows. */
const HISTORY_SECONDS = 60;
const TOAST_MS = 4000;

export interface Toast {
  id: number;
  kind: "info" | "error";
  text: string;
}

const NO_TRAFFIC: Traffic = { up: 0, down: 0, upSpeed: 0, downSpeed: 0 };

/** The window's width from which the sidebar and the two-pane connection
 * screen are used (styles.css: --breakpoint-nav). */
const WIDE_QUERY = "(min-width: 900px)";

export const useAppStore = defineStore("app", () => {
  // The backend owns the state and sends it whole on every change.
  const snapshot = shallowRef<Snapshot | null>(null);
  const traffic = ref<Traffic>(NO_TRAFFIC);
  /** The last minute of speeds, one a second, for the chart; kept here so it
   * survives going to another page, started over with each connection. */
  const history = shallowRef<{ down: number[]; up: number[] }>({ down: [], up: [] });
  let historyOf: number | null = null;
  const logs = ref<LogLine[]>([]);
  const toasts = ref<Toast[]>([]);
  let nextToast = 1;
  /** A subscription a uniray:// link offered; the servers page opens the
   * import dialog with it, so the user confirms (any web page can open such
   * a link). */
  const pendingImport = ref<string | null>(null);

  const servers = computed(() => snapshot.value?.servers ?? []);
  const subscriptions = computed(() => snapshot.value?.subscriptions ?? []);
  const settings = computed(() => snapshot.value?.settings ?? null);
  const connection = computed(() => snapshot.value?.connection ?? null);
  const status = computed(() => connection.value?.status ?? "disconnected");
  const isDesktop = computed(() => !["android", "ios"].includes(snapshot.value?.platform ?? ""));
  /** Android TV: D-pad navigation and the 10-foot layout. */
  const isTv = computed(() => snapshot.value?.tv === true);
  const wideQuery = window.matchMedia(WIDE_QUERY);
  const wideWindow = ref(wideQuery.matches);
  wideQuery.addEventListener("change", (event) => (wideWindow.value = event.matches));
  /** A wide desktop window: the connection and the servers on one screen.
   * Narrow windows, phones and TVs have them as two tabs. */
  const wide = computed(() => wideWindow.value && !isTv.value);
  /** TUN needs the elevated helper: pkexec on Linux, UAC on Windows, the
   * administrator prompt on macOS. */
  const tunSupported = computed(() => ["linux", "windows", "macos"].includes(snapshot.value?.platform ?? ""));

  /** The server a connection would use: the selected one, else the first. */
  const selectedServer = computed<Server | null>(() => {
    const selected = settings.value?.selectedServer;
    return servers.value.find((server) => server.id === selected) ?? servers.value[0] ?? null;
  });

  function apply(next: Snapshot) {
    snapshot.value = next;
    i18n.global.locale.value = resolveLocale(next.settings.locale);
    if (next.connection.status !== "connected") {
      traffic.value = NO_TRAFFIC;
    }
    const since = next.connection.status === "connected" ? next.connection.since : null;
    if (since !== historyOf) {
      historyOf = since;
      history.value = { down: [], up: [] };
    }
  }

  async function init() {
    await events.state(apply);
    await events.traffic((next) => {
      traffic.value = next;
      if (status.value !== "connected") return;
      const { down, up } = history.value;
      history.value = {
        down: [...down.slice(1 - HISTORY_SECONDS), next.downSpeed],
        up: [...up.slice(1 - HISTORY_SECONDS), next.upSpeed],
      };
    });
    await events.log((line) => {
      logs.value.push(line);
      if (logs.value.length > MAX_LOG_LINES) {
        logs.value.splice(0, logs.value.length - MAX_LOG_LINES);
      }
    });
    apply(await api.getState());
    logs.value = await api.getLogs();
  }

  function toast(text: string, kind: Toast["kind"] = "info") {
    const id = nextToast++;
    toasts.value.push({ id, kind, text });
    setTimeout(() => dismiss(id), TOAST_MS);
  }

  function dismiss(id: number) {
    toasts.value = toasts.value.filter((item) => item.id !== id);
  }

  /** The localized text of a backend error, with its detail when it adds any. */
  function describe(error: CmdError): string {
    const { t, te } = i18n.global;
    const key = `errors.${error.code}`;
    const title = te(key) ? t(key) : t("errors.other");
    const detailed = ["corpVia", "core", "coreStopped", "teardown", "network", "http", "invalidLink", "invalidConfig", "invalidSettings", "other", "io"];
    return detailed.includes(error.code) && error.message ? `${title}: ${error.message}` : title;
  }

  /** Runs a backend call, turning a failure into a toast. */
  async function run<T>(call: () => Promise<T>): Promise<T | undefined> {
    try {
      return await call();
    } catch (err) {
      toast(describe(toCmdError(err)), "error");
      return undefined;
    }
  }

  async function toggleConnection() {
    if (status.value === "disconnected") {
      // A failed attempt is shown on the home screen from the connection state.
      await api.connect().catch(() => undefined);
    } else if (status.value === "connected") {
      await api.disconnect();
    }
  }

  /** Acts on the uniray:// links that arrived (launcher shortcuts, links from
   * a browser); see src-tauri/src/deeplink.rs. */
  async function takeDeepLinks(): Promise<number> {
    const links = await api.takeDeepLinks().catch(() => []);
    for (const link of links) {
      if (link.action === "import") {
        pendingImport.value = link.url;
      } else if (link.action === "toggle") {
        await toggleConnection();
      } else if (link.action === "connect" && status.value === "disconnected") {
        await toggleConnection();
      } else if (link.action === "disconnect" && status.value === "connected") {
        await toggleConnection();
      }
    }
    return links.length;
  }

  async function saveSettings(patch: Partial<Settings>) {
    if (!settings.value) return;
    await run(() => api.saveSettings({ ...settings.value!, ...patch }));
  }

  async function clearLogs() {
    await api.clearLogs();
    logs.value = [];
  }

  return {
    snapshot,
    servers,
    subscriptions,
    settings,
    connection,
    status,
    isDesktop,
    isTv,
    wide,
    tunSupported,
    selectedServer,
    traffic,
    history,
    logs,
    toasts,
    pendingImport,
    init,
    toast,
    dismiss,
    describe,
    run,
    toggleConnection,
    takeDeepLinks,
    saveSettings,
    clearLogs,
  };
});
