import { createI18n } from "vue-i18n";

import en from "./en";
import ru from "./ru";

export const LOCALES = ["ru", "en"] as const;
export type Locale = (typeof LOCALES)[number];

export function systemLocale(): Locale {
  return navigator.language.toLowerCase().startsWith("ru") ? "ru" : "en";
}

/** The locale to use for a stored preference, `null` following the system. */
export function resolveLocale(preference: string | null): Locale {
  return LOCALES.find((locale) => locale === preference) ?? systemLocale();
}

export const i18n = createI18n({
  legacy: false,
  locale: systemLocale(),
  fallbackLocale: "en",
  messages: { ru, en },
});
