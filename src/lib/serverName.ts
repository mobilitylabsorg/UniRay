// Server names as subscriptions write them, e.g. "🇩🇪 【⚗️】 Bokrug | Hysteria":
// a country flag, a tag in 【】 with an emoji (and sometimes words), the name,
// and after "|" what kind of server it is. The UI shows the flag as a round
// picture, the emoji in front of the name, and the rest under it.

const REGIONAL_A = 0x1f1e6;
const FLAG = /^([\u{1F1E6}-\u{1F1FF}]{2})\s*/u;
const GLOBE = /^🌐️?\s*/u;
const TAG = /^【\s*([^】]*?)\s*】\s*/u;
const EMOJI = /^(?:\p{Extended_Pictographic}|\p{Emoji_Presentation})(?:️|‍(?:\p{Extended_Pictographic}|\p{Emoji_Presentation}))*️?/u;

export interface ServerName {
  /** ISO country code from a leading flag, e.g. "DE". */
  country: string | null;
  /** A leading 🌐: a group of servers around the world (a balancer). */
  globe: boolean;
  /** The emoji of a 【】 tag, shown in front of the name. */
  emoji: string | null;
  /** The name itself. */
  title: string;
  /** What came after "|" and the tag's words: leads the subtitle. */
  details: string[];
}

export function parseServerName(raw: string): ServerName {
  let rest = raw.trim();
  let country: string | null = null;
  let globe = false;
  let emoji: string | null = null;
  let words = "";

  const flag = rest.match(FLAG);
  if (flag) {
    country = [...flag[1]].map((char) => String.fromCharCode(char.codePointAt(0)! - REGIONAL_A + 65)).join("");
    rest = rest.slice(flag[0].length);
  }
  const globeMatch = rest.match(GLOBE);
  if (globeMatch) {
    globe = true;
    rest = rest.slice(globeMatch[0].length);
  }
  const tag = rest.match(TAG);
  if (tag) {
    rest = rest.slice(tag[0].length);
    const inner = tag[1].trim();
    const leading = inner.match(EMOJI);
    emoji = leading ? leading[0] : null;
    words = (leading ? inner.slice(leading[0].length) : inner).trim();
  }

  const [first, ...after] = rest.split("|");
  let title = first.trim();
  const details: string[] = [];
  const kind = after.join("|").trim();
  if (!title && words) {
    // "【⚡ Обычные 】": the tag's words are the name.
    title = words;
  } else if (words) {
    details.push(words);
  }
  if (kind) details.push(kind);
  if (!title) title = raw.trim();
  return { country, globe, emoji, title, details };
}

/** The name as one line of text: "⚗️ Bokrug · Hysteria". */
export function nameLine(name: ServerName): string {
  const head = name.emoji ? `${name.emoji} ${name.title}` : name.title;
  return name.details.length ? `${head} · ${name.details[0]}` : head;
}

/**
 * Subtitle parts without repeats, case-insensitive; of two parts where one
 * starts with the other the longer stays ("Hysteria 2" over "hysteria", one
 * of "xHTTP" and "xhttp"). "none" is left out.
 */
export function dedupeParts(parts: (string | null | undefined)[]): string[] {
  const out: string[] = [];
  for (const part of parts) {
    const value = part?.trim();
    if (!value || value.toLowerCase() === "none") continue;
    const low = value.toLowerCase();
    const index = out.findIndex((seen) => seen.toLowerCase().startsWith(low) || low.startsWith(seen.toLowerCase()));
    if (index < 0) out.push(value);
    else if (value.length > out[index].length) out[index] = value;
  }
  return out;
}

/** Protocol names as people write them; i18n keys for the ones that are words. */
const PROTOCOLS: Record<string, string> = {
  vless: "VLESS",
  vmess: "VMess",
  trojan: "Trojan",
  shadowsocks: "Shadowsocks",
  hysteria: "Hysteria 2",
  wireguard: "WireGuard",
  amneziawg: "AmneziaWG",
  naive: "NaiveProxy",
};

/** "Hysteria 2" for "hysteria"; a balancer is translated by the caller. */
export function protocolName(protocol: string): string {
  return PROTOCOLS[protocol.toLowerCase()] ?? protocol;
}
