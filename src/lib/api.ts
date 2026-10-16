// Typed bridge to the Rust backend. The shapes mirror `uniray-core::model`
// and `src-tauri/src/app.rs`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface Server {
  id: string;
  name: string;
  protocol: string;
  address: string;
  port: number;
  transport: string;
  security: string;
  link: string | null;
  subscriptionId: string | null;
  /** Milliseconds; -1 when the test failed, null when not measured. */
  delayMs: number | null;
  warnings?: string[];
}

export interface UserInfo {
  upload: number;
  download: number;
  /** Bytes, 0 means unlimited. */
  total: number;
  /** Unix seconds, null means never. */
  expire: number | null;
}

export interface Subscription {
  id: string;
  name: string;
  url: string;
  updatedAt: number | null;
  updateIntervalHours: number | null;
  userInfo: UserInfo | null;
  hwidActive: boolean;
  supportUrl: string | null;
  webPageUrl: string | null;
  announce: string | null;
  lastError: string | null;
}

export type Mode = "proxy" | "tun";
export type RoutingPreset = "global" | "bypassLan" | "bypassRu";
export type RuleKind = "domain" | "ip";
export type RuleAction = "proxy" | "direct" | "block";
export type PerAppMode = "off" | "allow" | "disallow";
export type DelayTestMode = "http" | "tcp";

export interface Rule {
  id: string;
  kind: RuleKind;
  value: string;
  action: RuleAction;
  enabled: boolean;
}

export interface Settings {
  mode: Mode;
  mixedPort: number;
  allowLan: boolean;
  proxyAuth: boolean;
  proxyUser: string;
  proxyPassword: string;
  routingPreset: RoutingPreset;
  customRules: Rule[];
  remoteDns: string;
  directDns: string;
  /** Use the network's own (the provider's) DNS as the direct DNS. */
  directDnsFromNetwork: boolean;
  ipv6: boolean;
  sendHwid: boolean;
  logLevel: string;
  interfaceName: string;
  perApp: { mode: PerAppMode; packages: string[] };
  stealth: boolean;
  stealthRootTun: boolean;
  /** Android: keep the tunnel working with the screen off (a wake lock while connected). */
  keepAwake: boolean;
  blockAds: boolean;
  autoUpdateHours: number;
  selectedServer: string | null;
  locale: string | null;
  delayTestMode: DelayTestMode;
  probeUrl: string;
  tlsFragment: boolean;
  tlsFragmentLength: string;
  tlsFragmentInterval: string;
  tlsFragmentPackets: string;
  tlsFingerprint: string;
  userAgent: string;
  tcpFastOpen: boolean;
  allowInsecure: boolean;
  wgMtu: number;
  autoConnect: boolean;
  autoFailover: boolean;
  autoRecover: boolean;
}

/** Elevated access the device offers; only `root`/`shizukuRoot` can do stealth. */
export type PrivilegeLevel = "none" | "shizukuShell" | "shizukuRoot" | "root";

export interface PrivilegeState {
  level: PrivilegeLevel;
  shizukuAvailable: boolean;
  shizukuGranted: boolean;
  hasTproxy?: boolean;
}

export interface CmdError {
  code: string;
  message: string;
}

export type Status = "disconnected" | "connecting" | "connected" | "disconnecting";

export interface CheckResult {
  ok: boolean;
  /** One HTTPS request through the tunnel, connection setup included. */
  latencyMs: number | null;
  /** Exit address and its country code, as seen from the outside. */
  ip: string | null;
  country: string | null;
  /** The server a balancer group picked, with its real name. */
  server: string | null;
  error: string | null;
  /** Unix seconds. */
  at: number;
}

export interface Connection {
  status: Status;
  serverId: string | null;
  mode: Mode;
  since: number | null;
  error: CmdError | null;
  mixedPort: number | null;
  /** The last check of this connection; repeated every 5 minutes. */
  check: CheckResult | null;
}

export interface Snapshot {
  servers: Server[];
  subscriptions: Subscription[];
  settings: Settings;
  connection: Connection;
  device: { hwid: string; os: string; osVersion: string; model: string };
  version: string;
  platform: "linux" | "windows" | "macos" | "android" | "ios";
  /** Android TV / Google TV: the remote-control layout. */
  tv: boolean;
  testing: boolean;
}

export interface Traffic {
  up: number;
  down: number;
  upSpeed: number;
  downSpeed: number;
}

export interface LogLine {
  id: number;
  time: number;
  level: "debug" | "info" | "warning" | "error" | "access";
  text: string;
}

export interface InstalledApp {
  package: string;
  label: string;
  system: boolean;
}

/** UI color tokens (`bg`, `surface`, `accent`, ...) as `#rrggbb`. */
export type ColorTokens = Record<string, string>;

export interface SystemColors {
  light?: ColorTokens;
  dark?: ColorTokens;
}

export interface ImportReport {
  added: number;
  errors: string[];
}

export interface ServiceProbeResult {
  ok: boolean;
  status: number;
  latencyMs: number;
  error: string | null;
}

export interface SpeedTestLatencyResult {
  pingMs: number | null;
  jitterMs: number | null;
}

export interface UnderlyingNetworkInfo {
  type: string;
  dns: string[];
  address?: string | null;
}

export interface SpeedTestResult {
  bytes: number;
  elapsedMs: number;
  mbps: number;
}

export type DpiVerdict = "clear" | "blocked" | "unknown";

export interface DpiFinding {
  id: "tcp" | "tls_sni" | "sustained" | "dns" | "udp";
  title: string;
  verdict: DpiVerdict;
  detail: string;
}

export interface DpiReport {
  findings: DpiFinding[];
  canApply: boolean;
}

/** What a uniray:// link asks for (src-tauri/src/deeplink.rs). */
export type DeepLink =
  | { action: "connect" }
  | { action: "disconnect" }
  | { action: "toggle" }
  | { action: "import"; url: string };

export const api = {
  getState: () => invoke<Snapshot>("get_state"),
  /** The uniray:// links that arrived since the last call. */
  takeDeepLinks: () => invoke<DeepLink[]>("take_deep_links"),
  import: (text: string) => invoke<ImportReport>("import", { text }),
  /** TV: starts the import page for a phone; resolves its URL (for the QR code). */
  handoffStart: () => invoke<string>("handoff_start"),
  handoffStop: () => invoke<void>("handoff_stop"),
  deleteServers: (ids: string[]) => invoke<void>("delete_servers", { ids }),
  selectServer: (id: string) => invoke<void>("select_server", { id }),
  addSubscription: (url: string) => invoke<ImportReport>("add_subscription", { url }),
  updateSubscription: (id: string) => invoke<ImportReport>("update_subscription", { id }),
  updateAllSubscriptions: () => invoke<ImportReport[]>("update_all_subscriptions"),
  deleteSubscription: (id: string) => invoke<void>("delete_subscription", { id }),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  connect: () => invoke<void>("connect"),
  disconnect: () => invoke<void>("disconnect"),
  testDelays: (ids?: string[]) => invoke<void>("test_delays", { ids: ids ?? null }),
  checkConnection: () => invoke<CheckResult>("check_connection"),
  listApps: () => invoke<InstalledApp[]>("list_apps"),
  systemColors: () => invoke<SystemColors | null>("system_colors"),
  privilegeState: () => invoke<PrivilegeState>("privilege_state"),
  requestShizuku: () => invoke<PrivilegeState>("request_shizuku"),
  /** Applies keepAwake to the running tunnel; turning it on asks for the battery exemption. */
  setKeepAwake: (enabled: boolean) => invoke<{ batteryExempt: boolean }>("set_keep_awake", { enabled }),
  getLogs: () => invoke<LogLine[]>("get_logs"),
  clearLogs: () => invoke<void>("clear_logs"),
  exportBackup: () => invoke<string>("export_backup"),
  importBackup: (json: string, mode: "merge" | "replace") => invoke<void>("import_backup", { json, mode }),
  probeService: (url: string) => invoke<ServiceProbeResult>("probe_service", { url }),
  speedTestDownload: (bytes: number) => invoke<SpeedTestResult>("speed_test_download", { bytes }),
  speedTestUpload: (bytes: number) => invoke<SpeedTestResult>("speed_test_upload", { bytes }),
  speedTestMultiStreamDownload: (durationSec?: number, url?: string, bypassTunnel?: boolean) =>
    invoke<SpeedTestResult>("speed_test_multistream_download", { durationSec, url, bypassTunnel }),
  speedTestMultiStreamUpload: (durationSec?: number, url?: string, bypassTunnel?: boolean) =>
    invoke<SpeedTestResult>("speed_test_multistream_upload", { durationSec, url, bypassTunnel }),
  speedTestProbeLatency: (url: string, samples?: number, bypassTunnel?: boolean) =>
    invoke<SpeedTestLatencyResult>("speed_test_probe_latency", { url, samples, bypassTunnel }),
  getUnderlyingNetwork: () => invoke<UnderlyingNetworkInfo>("get_underlying_network"),
  runDpiProbe: () => invoke<DpiReport>("run_dpi_probe"),
  applyDpiFixes: () => invoke<void>("apply_dpi_fixes"),
  quit: () => invoke<void>("quit"),
};

export const events = {
  state: (handler: (snapshot: Snapshot) => void): Promise<UnlistenFn> =>
    listen<Snapshot>("state", (event) => handler(event.payload)),
  traffic: (handler: (traffic: Traffic) => void): Promise<UnlistenFn> =>
    listen<Traffic>("traffic", (event) => handler(event.payload)),
  log: (handler: (line: LogLine) => void): Promise<UnlistenFn> =>
    listen<LogLine>("log", (event) => handler(event.payload)),
  /** Something sent from a phone to the TV's import page was imported. */
  handoff: (handler: (report: ImportReport) => void): Promise<UnlistenFn> =>
    listen<ImportReport>("handoff", (event) => handler(event.payload)),
  /** A uniray:// link arrived; take it with `api.takeDeepLinks`. */
  deepLink: (handler: () => void): Promise<UnlistenFn> => listen("deep-link", () => handler()),
};

/** Commands reject with a `CmdError`; anything else is an unexpected failure. */
export function toCmdError(err: unknown): CmdError {
  if (typeof err === "object" && err !== null && "code" in err && "message" in err) {
    return err as CmdError;
  }
  return { code: "other", message: String(err) };
}
