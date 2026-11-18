#!/usr/bin/env python3
"""Generates Tomi/Resources/Localizable.xcstrings from the Android strings.

Every text shown by Tomi lives once, in apps/android/app/src/main/res/values/strings.xml
(French, informal, "Masqué" / "Afficher"). The Apple apps use the same keys (`action_copy`,
`banner_sent`...) and the same texts, converted to a String Catalog. Run it again after any
change of the Android strings (deterministic output).

Conversions: `%1$s` -> `%1$@`, `%1$d` -> `%1$lld` (and the unnumbered forms), Android escapes
(`\\'`, `\\"`, `\\n`, `\\t`, `\\@`, `\\?`) resolved, `<plurals>` -> plural variations.

Texts of the Apple apps only (`ios_` keys) come from tools/strings-apple.json.

Usage: python3 -I apps/apple/tools/strings-from-android.py
"""

import json
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "apps/android/app/src/main/res/values/strings.xml"
OUTPUT = ROOT / "apps/apple/Tomi/Resources/Localizable.xcstrings"
LANGUAGE = "fr"
APPLE_ONLY = Path(__file__).resolve().parent / "strings-apple.json"


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


def main() -> int:
    root = ET.parse(SOURCE).getroot()
    strings = {}
    for el in root:
        name = el.get("name")
        if not name or el.get("translatable") == "false":
            continue
        if el.tag == "string":
            localization = unit(convert("".join(el.itertext())))
        elif el.tag == "plurals":
            variations = {}
            for item in el.findall("item"):
                variations[item.get("quantity")] = unit(convert("".join(item.itertext())))
            localization = {"variations": {"plural": variations}}
        else:
            continue
        strings[name] = {
            "extractionState": "manual",
            "localizations": {LANGUAGE: localization},
        }
    # Texts of the Apple apps only (`ios_` keys), kept next to this script.
    apple = json.loads(APPLE_ONLY.read_text(encoding="utf-8"))
    for name, value in apple.items():
        if name.startswith("_"):
            continue
        if not name.startswith("ios_") or name in strings:
            raise SystemExit(f"{APPLE_ONLY.name}: {name} must start with ios_ and not exist on Android")
        strings[name] = {"extractionState": "manual", "localizations": {LANGUAGE: unit(value)}}
    catalog = {"sourceLanguage": LANGUAGE, "strings": strings, "version": "1.0"}
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(catalog, ensure_ascii=False, indent=2, sort_keys=True, separators=(",", " : "))
    OUTPUT.write_text(text + "\n", encoding="utf-8")
    print(f"wrote {OUTPUT.relative_to(ROOT)} ({len(strings)} keys)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
