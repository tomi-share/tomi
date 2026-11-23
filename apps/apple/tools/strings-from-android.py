#!/usr/bin/env python3
"""Generates Tomi/Resources/Localizable.xcstrings from the Android strings.

Every text shown by Tomi lives once per language, in the Android resources:
apps/android/app/src/main/res/values/strings.xml (English, the reference and the fallback) and
res/values-<lang>/strings.xml for each other language. The Apple apps use the same keys (`action_copy`,
`banner_sent`...) and the same texts, converted to one String Catalog holding every language. Run it again after
any change of the Android strings (deterministic output).

Conversions: `%1$s` -> `%1$@`, `%1$d` -> `%1$lld` (and the unnumbered forms), Android escapes
(`\\'`, `\\"`, `\\n`, `\\t`, `\\@`, `\\?`) resolved, `<plurals>` -> plural variations.

Texts of the Apple apps only (`ios_` keys) come from tools/strings-apple/<lang>.json. A key missing in a language
falls back to English in the app; a key that English does not have is refused here.

The sentences the system shows when it asks for a permission (camera, Face ID, Photos) are in the same files, under
`_system`: they go to Tomi/Resources/InfoPlist.xcstrings. Their English is also the value of the build setting in
project.yml (what a device in a language the app does not have is shown); the two are checked to be the same.

Usage: python3 -I apps/apple/tools/strings-from-android.py
"""

import json
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RESOURCES = ROOT / "apps/android/app/src/main/res"
OUTPUT = ROOT / "apps/apple/Tomi/Resources/Localizable.xcstrings"
# The language of res/values: the reference, and what a device in a language the app does not have is shown.
REFERENCE = "en"
APPLE_ONLY = Path(__file__).resolve().parent / "strings-apple"
PROJECT = ROOT / "apps/apple/project.yml"


def languages() -> dict:
    """Language tag -> strings.xml. `values-pt-rBR` is the Android spelling of `pt-BR`, `values-b+zh+Hans` of `zh-Hans`."""
    found = {REFERENCE: RESOURCES / "values" / "strings.xml"}
    for folder in sorted(RESOURCES.glob("values-*")):
        source = folder / "strings.xml"
        if not source.exists():
            continue
        qualifier = folder.name[len("values-"):]
        if qualifier.startswith("b+"):
            tag = qualifier[2:].replace("+", "-")
        else:
            tag = re.sub(r"-r([A-Z]{2})$", r"-\1", qualifier)
        # Android still spells Indonesian and Hebrew with their old codes.
        found[{"in": "id", "iw": "he"}.get(tag, tag)] = source
    return found


# The name of a language in the catalog when Apple wants more than the language code: `zh` here is Simplified Chinese.
CATALOG_TAGS = {"zh": "zh-Hans"}


def convert(text: str) -> str:
    text = text.strip() if text else ""
    # Android unescaping (resources keep `\'`, `\"`, `\n`... literally).
    out, i = [], 0
    while i < len(text):
        c = text[i]
        if c == "\\" and i + 1 < len(text):
            nxt = text[i + 1]
            out.append({"n": "\n", "t": "\t"}.get(nxt, nxt))
            i += 2
            continue
        out.append(c)
        i += 1
    text = "".join(out)
    text = re.sub(r"%(\d+\$)?s", lambda m: f"%{m.group(1) or ''}@", text)
    text = re.sub(r"%(\d+\$)?d", lambda m: f"%{m.group(1) or ''}lld", text)
    return text


def unit(value: str) -> dict:
    return {"stringUnit": {"state": "translated", "value": value}}


def read_android(source: Path) -> dict:
    """Key -> localization (a string unit, or plural variations) of one strings.xml."""
    out = {}
    for el in ET.parse(source).getroot():
        name = el.get("name")
        if not name or el.get("translatable") == "false":
            continue
        if el.tag == "string":
            out[name] = unit(convert("".join(el.itertext())))
        elif el.tag == "plurals":
            variations = {}
            for item in el.findall("item"):
                variations[item.get("quantity")] = unit(convert("".join(item.itertext())))
            out[name] = {"variations": {"plural": variations}}
    return out


def read_apple(language: str, android: dict) -> dict:
    """Texts of the Apple apps only (`ios_` keys), kept next to this script, one file per language."""
    source = APPLE_ONLY / f"{language}.json"
    if not source.exists():
        return {}
    out = {}
    for name, value in json.loads(source.read_text(encoding="utf-8")).items():
        if name.startswith("_"):
            continue
        if not name.startswith("ios_") or name in android:
            raise SystemExit(f"{source.name}: {name} must start with ios_ and not exist on Android")
        out[name] = unit(value)
    return out


def write_catalog(output: Path, strings: dict) -> None:
    catalog = {"sourceLanguage": REFERENCE, "strings": strings, "version": "1.0"}
    output.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(catalog, ensure_ascii=False, indent=2, sort_keys=True, separators=(",", " : "))
    output.write_text(text + "\n", encoding="utf-8")
    print(f"wrote {output.relative_to(ROOT)} ({len(strings)} keys, {len(languages())} languages)")


def system_texts(section: str, output: Path) -> None:
    """Writes the catalog of the permission sentences (Info.plist keys) from `section` of each language file."""
    strings = {}
    project = PROJECT.read_text(encoding="utf-8")
    for language in languages():
        source = APPLE_ONLY / f"{language}.json"
        texts = json.loads(source.read_text(encoding="utf-8")).get(section, {}) if source.exists() else {}
        for name, value in texts.items():
            if language == REFERENCE and f'INFOPLIST_KEY_{name}: "{value}"' not in project:
                raise SystemExit(f"project.yml: INFOPLIST_KEY_{name} must be the English text of {section}")
            if language != REFERENCE and name not in strings:
                raise SystemExit(f"{source.name}: {section} names {name}, which {REFERENCE} does not have")
            entry = strings.setdefault(name, {"extractionState": "manual", "localizations": {}})
            entry["localizations"][CATALOG_TAGS.get(language, language)] = unit(value)
    write_catalog(output, strings)


def main() -> int:
    strings = {}
    reference = None
    for language, source in languages().items():
        android = read_android(source)
        texts = {**android, **read_apple(language, android)}
        if language == REFERENCE:
            reference = set(texts)
        else:
            unknown = sorted(set(texts) - reference)
            if unknown:
                raise SystemExit(f"{language}: keys that {REFERENCE} does not have: {', '.join(unknown[:5])}")
        for name, localization in texts.items():
            entry = strings.setdefault(name, {"extractionState": "manual", "localizations": {}})
            entry["localizations"][CATALOG_TAGS.get(language, language)] = localization
    write_catalog(OUTPUT, strings)
    system_texts("_system", OUTPUT.with_name("InfoPlist.xcstrings"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
