# Localization

The user interface follows the language of the device on every platform. **English is the reference and the
fallback**: a device in a language Tomi does not have shows English. Code, comments and documentation are in English.

Strings that must stay language-independent: protocol labels, error codes and state names (see `spec/`), and the pairing word
list (French, part of the pairing code format — `spec/vectors/pairing-v1/wordlist.json`).

## Vocabulary

Product words are translated once and used everywhere. In English: space, relay, device, feed, pairing, installation key,
**Hidden** / **Show** (a hidden item; never "secret" / "reveal"), Pin / Pinned, Archive / Archives, Recent. Plain "you",
sentence case, no exclamation marks. Security warnings keep their exact strength ("cannot be undone", "permanent").
A translation follows the same choices in its own language and keeps every placeholder.

## Android

- `apps/android/app/src/main/res/values/strings.xml` is English; each other language is
  `res/values-<lang>/strings.xml` with the same keys. A key missing in a language falls back to English.
- To add a language: create `res/values-<lang>/strings.xml`, and add a `<locale>` line to `res/xml/locales_config.xml`
  (the per-app language setting of Android lists what is declared there). Plurals carry the quantities the language
  needs (`one`, `few`, `many`, `other`...); formatted strings keep the same placeholders.
- Dates and numbers use the device locale (`ui/common/Formatting.kt`).

## iOS

- `apps/apple/Tomi/Resources/Localizable.xcstrings` is **generated**, never edited: `python3 -I
  apps/apple/tools/strings-from-android.py` reads the Android files of every language and, for the texts only the
  Apple apps have (`ios_` keys), `apps/apple/tools/strings-apple/<lang>.json`.
- To add a language: add the Android file, add `tools/strings-apple/<lang>.json`, run the generator. The source
  language of the catalog and the development language of the project (`apps/apple/project.yml`) are English.
- Dates and sizes use the device locale (`TomiLocale` in TomiKit); texts are formatted with it (plural rules).
- The tests assert French texts: they pin the locale they need.

## Windows

- The webview reads `msg` from `apps/windows/src/lib/i18n/index.ts`, which picks a dictionary once from the system
  languages (exact tag, then the language alone, else English). `en.ts` is the reference: its shape is the `Dictionary`
  type. Plurals use the rules of the language (`Intl.PluralRules`), dates the picked locale.
- To add a language: write `i18n/<lang>.ts` exporting a `Dictionary`, and add it to `LOCALES` in `index.ts`.
- Strings produced on the Rust side (tray, toasts, Windows Hello prompts) are tables in
  `apps/windows/src-tauri/src/strings/<lang>.rs`, listed in `TABLES` (`strings/mod.rs`) and picked from the display
  languages of the user. The Tauri installer language list is in `src-tauri/tauri.conf.json`.

## Relay pages

The relay serves a few static HTML pages (landing, admin panel, creation page) with French text under `apps/relay/src/`.
They are not translated yet and have no locale mechanism.

## Not covered yet

- Right-to-left languages (Arabic, Hebrew): the layouts have not been checked for them.
- Translations other than French and English are machine-made and wait for a review by a native speaker.

## Tests

Several unit tests assert French UI strings. They pin the French locale (or table) explicitly; new tests should assert
on resource keys rather than literal text.
