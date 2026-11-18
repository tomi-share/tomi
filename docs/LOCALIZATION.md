# Localization

The user interface is available in **French only** for now; French is the default locale on every platform. Code,
comments and documentation are in English. This page explains how to add another language when the time comes.

Strings that must stay language-independent: protocol labels, error codes and state names (see `spec/`), and the pairing word
list (French, part of the pairing code format — `spec/vectors/pairing-v1/wordlist.json`).

## Android

- Strings live in `apps/android/app/src/main/res/values/strings.xml` (French, default).
- To add a language: create `res/values-<lang>/strings.xml` (for example `values-en`) with the same keys. Untranslated keys
  fall back to French. Plurals and formatted strings must keep the same placeholders.
- Date and number formatting uses the device locale.

## iOS

- Strings live in `apps/apple/Tomi/Resources/Localizable.xcstrings` (source language: French).
- To add a language: add it to the string catalog in Xcode (or add the `localizations` entries in the file) and to the
  project's known regions (`apps/apple/project.yml`). The share extension has its own resources.

## Windows

- Strings live in `apps/windows/src/lib/i18n/fr.ts` and are imported directly (`import { fr, format } from "./i18n/fr"`) by
  most components. There is **no locale selection layer yet**.
- To add a language, first introduce a small locale layer (a typed dictionary, an `i18n/index.ts` that picks the dictionary from
  the system or a setting, and a mechanical replacement of the direct `fr` imports), then add `i18n/<lang>.ts` implementing
  the same type. The Tauri installer language list is in `src-tauri/tauri.conf.json`.
- Tracked as an open task.

## Relay pages

The relay serves a few static HTML pages (landing, admin panel, creation page) with French text under `apps/relay/src/`.
They are not translated yet and have no locale mechanism.

## Tests

Several unit tests assert French UI strings. When adding a language, keep tests on the default (French) locale or assert on
resource keys rather than literal text.
