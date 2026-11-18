// Country flags as emoji: a flag is a pair of "regional indicator" symbols
// spelling the ISO 3166 country code.

const REGIONAL_A = 0x1f1e6;

/** 🇪🇪 for "EE"; empty for anything that is not a two-letter code. */
export function flagOf(countryCode: string | null | undefined): string {
  const code = (countryCode ?? "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(code)) return "";
  return String.fromCodePoint(...[...code].map((letter) => REGIONAL_A + letter.charCodeAt(0) - 65));
}
