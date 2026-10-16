/**
 * SpeedTest engine ported directly from Lean-main (SpeedTest.kt, SpeedTestScreen.kt, SpeedTestViewModel.kt).
 */

export interface SpeedTestServer {
  id: string;
  sponsor: string;
  place: string;
  host: string;
  distanceKm: number;
  kind: "ookla" | "cloudflare" | "librespeed";
}

export const BUILT_IN_TARGETS: SpeedTestServer[] = [
  { id: "selectel", sponsor: "Selectel", place: "Москва", host: "speedtest.selectel.ru", distanceKm: 0, kind: "librespeed" },
  { id: "46685", sponsor: "fdcservers.net", place: "Москва", host: "speedtest.dme.fdcservers.net:8080", distanceKm: 0, kind: "ookla" },
  { id: "58664", sponsor: "ProCloud", place: "Москва", host: "speedtest.procloud.ru:8080", distanceKm: 0, kind: "ookla" },
  { id: "12461", sponsor: "PG-19", place: "Таганрог", host: "speedtesttgn.pg19.ru:8080", distanceKm: 0, kind: "ookla" },
  { id: "27144", sponsor: "Yacolo", place: "Кемерово", host: "speedtest.yacolo.com:8080", distanceKm: 0, kind: "ookla" },
  { id: "40162", sponsor: "RIKT", place: "Междуреченск", host: "speedtest.rikt.ru.prod.hosts.ooklaserver.net:8080", distanceKm: 0, kind: "ookla" },
  { id: "46700", sponsor: "fdcservers.net", place: "Хабаровск", host: "speedtest.khv.fdcservers.net:8080", distanceKm: 0, kind: "ookla" },
  { id: "cloudflare", sponsor: "Cloudflare", place: "ближайший узел", host: "speed.cloudflare.com", distanceKm: 0, kind: "cloudflare" },
];

export function getServerLatencyUrl(server: SpeedTestServer): string {
  switch (server.kind) {
    case "ookla":
      return `https://${server.host}/speedtest/latency.txt`;
    case "cloudflare":
      return `https://${server.host}/__down?bytes=0`;
    case "librespeed":
      return `https://${server.host}/empty.php`;
  }
}

export function getServerDownloadUrl(server: SpeedTestServer): string {
  const nonce = Math.floor(Math.random() * 2147483647);
  switch (server.kind) {
    case "ookla":
      return `https://${server.host}/download?size=100000000&nocache=${nonce}`;
    case "cloudflare":
      return `https://${server.host}/__down?bytes=50000000`;
    case "librespeed":
      return `https://${server.host}/garbage.php?ckSize=100&nocache=${nonce}`;
  }
}

export function getServerUploadUrl(server: SpeedTestServer): string {
  switch (server.kind) {
    case "ookla":
      return `https://${server.host}/upload`;
    case "cloudflare":
      return `https://${server.host}/__up`;
    case "librespeed":
      return `https://${server.host}/empty.php`;
  }
}

export async function discoverOoklaServers(): Promise<SpeedTestServer[]> {
  try {
    const url = "https://www.speedtest.net/api/js/servers?engine=js&https_functional=true&limit=10";
    const res = await fetch(url);
    if (!res.ok) return [];
    const data = await res.json();
    if (!Array.isArray(data)) return [];
    const servers: SpeedTestServer[] = [];
    for (const item of data) {
      if (item.https_functional !== 1) continue;
      const host = item.host;
      if (!host) continue;
      servers.push({
        id: String(item.id || host),
        sponsor: item.sponsor || host.split(":")[0],
        place: [item.name, item.country].filter(Boolean).join(", "),
        host,
        distanceKm: Math.round(item.distance || 0),
        kind: "ookla",
      });
    }
    return servers;
  } catch {
    return [];
  }
}

export interface SpeedTestRunResult {
  downloadMbps: number | null;
  uploadMbps: number | null;
  pingMs: number | null;
  jitterMs: number | null;
  viaTunnel: boolean;
  server: SpeedTestServer;
  downloadNote?: string | null;
  uploadNote?: string | null;
  path?: string | null;
  atMillis: number;
}

export interface SpeedTestHistoryEntry {
  atMillis: number;
  serverName: string;
  downloadMbps: number | null;
  uploadMbps: number | null;
  pingMs: number | null;
  jitterMs: number | null;
  directDownloadMbps?: number | null;
  directUploadMbps?: number | null;
  endpointId: string;
  endpointLabel: string;
  direct: boolean;
}

const HISTORY_KEY = "uniray_speedtest_history";
const PINNED_KEY = "uniray_speedtest_pinned_target";
const SCOPE_KEY = "uniray_speedtest_scope";

export function loadHistory(): SpeedTestHistoryEntry[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

export function saveHistory(entries: SpeedTestHistoryEntry[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(entries.slice(0, 30)));
  } catch {}
}

export function loadPinnedServerId(): string | null {
  return localStorage.getItem(PINNED_KEY);
}

export function savePinnedServerId(id: string | null) {
  if (id) {
    localStorage.setItem(PINNED_KEY, id);
  } else {
    localStorage.removeItem(PINNED_KEY);
  }
}

export function loadSavedScope(): "tunnel" | "direct" | "both" {
  const val = localStorage.getItem(SCOPE_KEY);
  return val === "tunnel" || val === "direct" || val === "both" ? val : "both";
}

export function saveScope(scope: "tunnel" | "direct" | "both") {
  localStorage.setItem(SCOPE_KEY, scope);
}

// Dial math exactly matching Lean-main's SpeedTestScreen.kt
export const GAUGE_START_ANGLE = 150;
export const GAUGE_SWEEP = 240;
export const DIAL_STEPS = [10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0];
const DIAL_LABEL_VALUES = [1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0];
const LABEL_SLACK = 1.002;
const DIAL_CURVE = 0.6;

export function dialMax(peak: number): number {
  for (const step of DIAL_STEPS) {
    if (peak <= step * 0.92) return step;
  }
  return DIAL_STEPS[DIAL_STEPS.length - 1];
}

export function dialFraction(mbps: number, scaleMax: number): number {
  if (scaleMax <= 0) return 0;
  const ratio = Math.max(0, Math.min(1, mbps / scaleMax));
  return Math.pow(ratio, DIAL_CURVE);
}

export interface DialTick {
  fraction: number;
  label: string;
  major: boolean;
}

export function dialLabels(scaleMax: number): DialTick[] {
  const majors = DIAL_LABEL_VALUES.filter((it) => it <= scaleMax * LABEL_SLACK);
  const marks: DialTick[] = [];
  marks.push({ fraction: 0, label: "0", major: true });

  const uniqueMajors = Array.from(new Set(majors));
  for (const value of uniqueMajors) {
    const fraction = dialFraction(value, scaleMax);
    const label = value >= 1000 ? `${Math.round(value / 1000)}G` : `${Math.round(value)}`;
    marks.push({ fraction, label, major: true });
  }

  for (let index = 1; index < 16; index++) {
    const fraction = index / 16;
    if (!marks.some((m) => Math.abs(m.fraction - fraction) < 0.035)) {
      marks.push({ fraction, label: "", major: false });
    }
  }

  marks.sort((a, b) => a.fraction - b.fraction);
  return marks;
}

export interface FormattedRate {
  value: string;
  unit: string;
}

export function formatRate(mbps: number | null | undefined): FormattedRate {
  if (mbps === null || mbps === undefined) return { value: "—", unit: "Мбит/с" };
  if (mbps >= 1000) return { value: (mbps / 1000).toFixed(2), unit: "Гбит/с" };
  if (mbps >= 100) return { value: mbps.toFixed(0), unit: "Мбит/с" };
  if (mbps >= 10) return { value: mbps.toFixed(1), unit: "Мбит/с" };
  return { value: mbps.toFixed(2), unit: "Мбит/с" };
}

export function formatClock(atMillis: number): string {
  const d = new Date(atMillis);
  const h = String(d.getHours()).padStart(2, "0");
  const m = String(d.getMinutes()).padStart(2, "0");
  return `${h}:${m}`;
}
