// The locale layer of the webview: one dictionary per language (`en.ts`, `fr.ts`, ...), picked once at start from
// the languages of the system, and the helpers that format with it (plurals, dates, sizes). English is the
// reference and the fallback: a language the app does not have shows English.
//
// To add a language: write `<lang>.ts` exporting a `Dictionary`, and add it to `LOCALES` below.
// Strings produced on the Rust side (tray, toasts, Windows Hello prompts) live in `src-tauri/src/strings.rs`.

import type { AppError } from "../api";
import { en } from "./en";
import { de } from "./de";
import { es } from "./es";
import { fr } from "./fr";
import { hi } from "./hi";
import { id } from "./id";
import { it } from "./it";
import { ja } from "./ja";
import { ko } from "./ko";
import { nl } from "./nl";
import { pl } from "./pl";
import { pt } from "./pt";
import { ru } from "./ru";
import { tr } from "./tr";
import { zh } from "./zh";

/** The forms a count can take. Every language has `other`; English and French only add `one`. */
export type PluralForms = {
  one: string;
  other: string;
  zero?: string;
  two?: string;
  few?: string;
  many?: string;
};

type Widen<T> = T extends { one: string; other: string }
  ? PluralForms
  : T extends (...args: never[]) => unknown
    ? T
    : T extends object
      ? { [K in keyof T]: Widen<T[K]> }
      : T;

/** What every language implements: the shape of the English dictionary. */
export type Dictionary = Widen<typeof en>;

/** The languages of the app, by BCP 47 tag. The key order is the order shown wherever they are listed. */
export const LOCALES = { en, de, es, fr, hi, id, it, ja, ko, nl, pl, pt, ru, tr, zh } satisfies Record<string, Dictionary>;

export type Locale = keyof typeof LOCALES;

export const FALLBACK: Locale = "en";

/**
 * The language to show for someone whose preferred languages are `preferred` (most wanted first): the first one the
 * app has, exactly (`pt-BR`) or by its language alone (`fr-CA` → `fr`), else the fallback.
 */
export function pickLocale<T extends string>(preferred: readonly string[], available: readonly T[], fallback: T): T {
  for (const tag of preferred) {
    const wanted = tag.toLowerCase();
    const exact = available.find((a) => a.toLowerCase() === wanted);
    if (exact) return exact;
    const language = wanted.split("-")[0];
    const sameLanguage = available.find((a) => a.toLowerCase() === language);
    if (sameLanguage) return sameLanguage;
  }
  return fallback;
}

function systemLanguages(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages?.length ? navigator.languages : [navigator.language];
}

/** The language of this run (the webview follows the display language of Windows). */
export const locale: Locale = pickLocale(systemLanguages(), Object.keys(LOCALES) as Locale[], FALLBACK);

/** Every string shown in the webview, in the language of this run. */
export const msg: Dictionary = LOCALES[locale];

/** `format("from {device}", { device: "Pixel" })` */
export function format(template: string, params: Record<string, unknown> = {}): string {
  return template.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? ""));
}

const NAME_LIST = new Intl.ListFormat(locale, { style: "long", type: "conjunction" });

/**
 * The one-time notice of devices the relay removed on its own: one sentence per cause (the
 * administrator, the device itself, anything else), several devices grouped in a sentence
 * ("A", "A and B", "A, B and C" as the language writes a list).
 */
export function relayNoticeText(list: { name: string; via: "admin" | "self" | "other" }[]): string {
  const sentences: string[] = [];
  for (const via of ["admin", "self", "other"] as const) {
    const names = [...new Set(list.filter((r) => r.via === via).map((r) => r.name))];
    if (names.length === 0) continue;
    const forms = {
      admin: [msg.relayNotice.admin, msg.relayNotice.adminMany],
      self: [msg.relayNotice.self, msg.relayNotice.selfMany],
      other: [msg.relayNotice.other, msg.relayNotice.otherMany],
    }[via];
    sentences.push(format(forms[names.length === 1 ? 0 : 1], { names: NAME_LIST.format(names) }));
  }
  return sentences.join(" ");
}

const PLURAL_RULES = new Intl.PluralRules(locale);

/** The form of the language for `n`: `plural({ one: "{n} day", other: "{n} days" }, 3)`. */
export function plural(forms: PluralForms, n: number): string {
  return format(forms[PLURAL_RULES.select(n)] ?? forms.other, { n });
}

export function errorMessage(err: AppError | string): string {
  const code = typeof err === "string" ? err : err.code;
  const params = typeof err === "string" ? {} : err.params;
  const template = msg.errors[code];
  return template ? format(template, params) : msg.unknownError;
}

/**
 * The message of the error that ended a pairing. A wrong code is told differently to the device that typed the
 * code and to the one that showed it (spec/pairing-v1.md §11.5).
 */
export function pairingErrorMessage(err: AppError | null, role: "invite" | "join"): string {
  if (!err) return "";
  if (err.code === "error.pairing_failed") {
    return role === "join" ? msg.pairing.failedJoin : msg.pairing.failedInvite;
  }
  return errorMessage(err);
}

const ONE_DECIMAL = new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
const NO_DECIMAL = new Intl.NumberFormat(locale, { maximumFractionDigits: 0 });

/** A size, with one decimal under 10 units, written as the language writes numbers (`1.5 MB`, `1,5 Mo`). */
export function formatBytes(n: number): string {
  const u = msg.units;
  if (n < 1024) return format(u.bytes, { n });
  const scaled = (value: number) => (value < 10 ? ONE_DECIMAL : NO_DECIMAL).format(value);
  if (n < 1024 * 1024) return format(u.kilobytes, { n: scaled(n / 1024) });
  return format(u.megabytes, { n: scaled(n / 1024 / 1024) });
}

/** A short delay, `1 min 42 s` or `42 s`, rounded up to the second. */
export function formatRemaining(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return minutes > 0
    ? format(msg.units.minutesSeconds, { m: minutes, s: seconds })
    : format(msg.units.seconds, { s: seconds });
}

/** Relative time, past or future. */
export function relativeTime(ts: number, now = Date.now()): string {
  const diff = ts - now;
  const abs = Math.abs(diff);
  const min = Math.round(abs / 60_000);
  const t = msg.time;
  if (diff <= 0) {
    if (min < 1) return t.now;
    if (min < 60) return format(t.minutes, { n: min });
    const h = Math.round(min / 60);
    if (h < 24) return format(t.hours, { n: h });
    return format(t.days, { n: Math.round(h / 24) });
  }
  if (min < 60) return format(t.inMinutes, { n: Math.max(min, 1) });
  const h = Math.round(min / 60);
  if (h < 48) return format(t.inHours, { n: h });
  return format(t.inDays, { n: Math.round(h / 24) });
}

const TIME = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" });
const DAY = new Intl.DateTimeFormat(locale, { day: "numeric", month: "short" });
const DAY_YEAR = new Intl.DateTimeFormat(locale, { day: "numeric", month: "short", year: "numeric" });
const DATE_TIME = new Intl.DateTimeFormat(locale, {
  day: "numeric",
  month: "long",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});
const DATE_TIME_SECONDS = new Intl.DateTimeFormat(locale, {
  day: "numeric",
  month: "long",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

/** The time today, "Yesterday", the day this year, the day and the year before (like Android). */
export function formatTimestamp(ts: number, now = Date.now()): string {
  const d = new Date(ts);
  const n = new Date(now);
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((startOf(n) - startOf(d)) / 86_400_000);
  if (days <= 0) return TIME.format(d);
  if (days === 1) return msg.card.yesterday;
  return (d.getFullYear() === n.getFullYear() ? DAY : DAY_YEAR).format(d);
}

/** The date and the time in full, for the detail screen. */
export function formatDateTime(ts: number): string {
  return DATE_TIME.format(new Date(ts));
}

/** The same with the seconds: the exact send date, for the detail screen. */
export function formatPreciseDateTime(ts: number): string {
  return DATE_TIME_SECONDS.format(new Date(ts));
}

/** `3 days`, `2 hours`, `1 minute`: time left before an item expires. */
export function formatRemainingTime(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return plural(msg.duration.minutes, minutes);
  const hours = Math.round(minutes / 60);
  if (hours < 48) return plural(msg.duration.hours, hours);
  return plural(msg.duration.days, Math.round(hours / 24));
}
