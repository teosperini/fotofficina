#!/usr/bin/env python3
"""Rename LightCraft to Fotofficina everywhere the user can see it. Safe to run again and again.

Why a script: LightCraft changes every day and keeps adding new strings that say "LightCraft". Instead of
fixing conflicts by hand after each update, update first and then run this (tools/rebrand.py), which
re-applies the same rules to the new code.

Rules (what is renamed):
  * text inside string literals of the Rust code (crates/, apps/), but not comments;
  * both the keys and the translations of the locale catalogs (crates/ui-egui/locales/*.json);
  * the display names in the packaging files.
What is never renamed:
  * the credit to the original project (strings that say "based on", or link to storytold/);
  * names glued to other letters (LightCraftVersion...), crate/binary names (lowercase `lightcraft`),
    LIGHTCRAFT_* environment variables, documentation, comments, Cargo manifests, LICENSE/NOTICE/README.

    python3 tools/rebrand.py            # apply
    python3 tools/rebrand.py --check    # only report how many files would change (exit 1 if any)
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OLD, NEW = "LightCraft", "Fotofficina"
# not glued to other letters, except after an escape like \n or \t
NAME = re.compile(r"(?:(?<![A-Za-z])|(?<=\\[nt]))LightCraft(?![A-Za-z])")
STRING = re.compile(r'"(?:[^"\\\n]|\\.)*"')
KEEP = ("storytold", "based on", "Based on")  # credits / links to the original project

PACKAGING = [
    "packaging/macos/package.sh",
    "packaging/macos/Info.plist.in",
    "packaging/windows/lightcraft.wxs",
    "packaging/linux/ai.storyteller.lightcraft.metainfo.xml.in",
    "packaging/linux/nfpm.yaml",
    "nix/package.nix",
]


def rename_text(text):
    return NAME.sub(NEW, text)


def rename_literals(line):
    return STRING.sub(lambda m: m.group(0) if any(k in m.group(0) for k in KEEP) else rename_text(m.group(0)), line)


def process_rust(path):
    out, changed = [], False
    for line in path.read_text(encoding="utf-8").split("\n"):
        stripped = line.lstrip()
        new = line if stripped.startswith("//") or OLD not in line else rename_literals(line)
        changed |= new != line
        out.append(new)
    return "\n".join(out) if changed else None


def process_locale(path):
    out, changed = [], False
    for line in path.read_text(encoding="utf-8").split("\n"):
        new = line if (OLD not in line or any(k in line for k in KEEP)) else rename_text(line)
        changed |= new != line
        out.append(new)
    if not changed:
        return None
    text = "\n".join(out)
    json.loads(text)  # must stay valid JSON
    return text


def process_plain(path):
    text = path.read_text(encoding="utf-8")
    new = rename_text(text)
    return new if new != text else None


def main():
    check = "--check" in sys.argv
    jobs = []
    for folder in ("crates", "apps"):
        jobs += [(p, process_rust) for p in (ROOT / folder).rglob("*.rs") if "target" not in p.parts]
    jobs += [(p, process_locale) for p in (ROOT / "crates/ui-egui/locales").glob("*.json")]
    jobs += [(ROOT / rel, process_plain) for rel in PACKAGING if (ROOT / rel).exists()]

    changed = []
    for path, fn in jobs:
        new = fn(path)
        if new is not None:
            changed.append(path.relative_to(ROOT))
            if not check:
                path.write_text(new, encoding="utf-8")
    verb = "would change" if check else "changed"
    print(f"rebrand: {verb} {len(changed)} file(s)")
    for p in changed[:200]:
        print("  ", p)
    return 1 if (check and changed) else 0


if __name__ == "__main__":
    sys.exit(main())
