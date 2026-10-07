// Country flags as emoji: a flag is a pair of "regional indicator" symbols
// spelling the ISO 3166 country code.

const REGIONAL_A = 0x1f1e6;

/** 🇪🇪 for "EE"; empty for anything that is not a two-letter code. */
export function flagOf(countryCode: string | null | undefined): string {
  const code = (countryCode ?? "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(code)) return "";
  return String.fromCodePoint(...[...code].map((letter) => REGIONAL_A + letter.charCodeAt(0) - 65));
}

/** Whether `text` starts with a country flag (servers are usually named so). */
export function startsWithFlag(text: string): boolean {
  const first = text.trimStart().codePointAt(0) ?? 0;
  return first >= REGIONAL_A && first <= REGIONAL_A + 25;
}

/**
 * The server's name with a country flag in front: its own flag when it has
 * one, else the flag of the country the traffic actually leaves from (a
 * balancer, named with 🌐 or nothing, says where it ended up this way).
 */
export function withFlag(name: string, exitCountry: string | null | undefined): string {
  if (startsWithFlag(name)) return name;
  const flag = flagOf(exitCountry);
  // Drop a generic globe so the real flag takes its place.
  const bare = name.replace(/^\s*🌐\s*/u, "");
  return flag ? `${flag} ${bare}` : name;
}
