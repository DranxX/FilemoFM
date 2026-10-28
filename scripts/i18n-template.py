#!/usr/bin/env python3
"""Writes a translation template with every English source string used through tr(), tr!() or n_().

Usage: python3 scripts/i18n-template.py locales/id.toml
Existing translations in the target file are kept; new strings are added with an empty value,
which the app treats as "not translated yet" and shows in English.
"""
import pathlib
import re
import sys

SOURCE = pathlib.Path(__file__).resolve().parent.parent / "src"
PATTERN = re.compile(r'\b(?:tr!?|n_)\(\s*"((?:[^"\\]|\\.)*)"')


def toml_key(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    target = pathlib.Path(sys.argv[1])
    keys = set()
    for path in SOURCE.rglob("*.rs"):
        for match in PATTERN.finditer(path.read_text(encoding="utf-8")):
            keys.add(bytes(match.group(1), "utf-8").decode("unicode_escape"))
    existing = {}
    if target.exists():
        import tomllib

        existing = tomllib.loads(target.read_text(encoding="utf-8"))
    lines = [
        "# FilemoFM translation. Keys are the English source text; leave a value empty to keep English.",
        f'"@language-name" = {toml_key(existing.get("@language-name", ""))}',
        "",
    ]
    for key in sorted(keys):
        lines.append(f"{toml_key(key)} = {toml_key(existing.get(key, ''))}")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(keys)} strings written to {target}")


if __name__ == "__main__":
    main()
