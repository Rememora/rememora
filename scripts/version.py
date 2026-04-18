#!/usr/bin/env python3
"""Manage Rememora's single project-wide version.

Usage:
  scripts/version.py --check [--tag v1.2.3]
  scripts/version.py set 1.2.3
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def read(path: str) -> str:
    return (ROOT / path).read_text()


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text)


def version_file() -> str:
    return read("VERSION").strip()


def cargo_toml_version() -> str:
    match = re.search(
        r'(?ms)^\[package\]\s+name\s*=\s*"rememora"\s+version\s*=\s*"([^"]+)"',
        read("Cargo.toml"),
    )
    if not match:
        raise RuntimeError("could not find rememora package version in Cargo.toml")
    return match.group(1)


def cargo_lock_version() -> str:
    match = re.search(
        r'(?m)^\[\[package\]\]\nname = "rememora"\nversion = "([^"]+)"',
        read("Cargo.lock"),
    )
    if not match:
        raise RuntimeError("could not find rememora package version in Cargo.lock")
    return match.group(1)


def plugin_manifest_version() -> str:
    data = json.loads(read("plugin/.claude-plugin/plugin.json"))
    return data["version"]


def marketplace_version() -> str:
    data = json.loads(read(".claude-plugin/marketplace.json"))
    for plugin in data.get("plugins", []):
        if plugin.get("name") == "rememora":
            return plugin["version"]
    raise RuntimeError("could not find rememora plugin in .claude-plugin/marketplace.json")


def versions() -> dict[str, str]:
    return {
        "VERSION": version_file(),
        "Cargo.toml": cargo_toml_version(),
        "Cargo.lock": cargo_lock_version(),
        "plugin/.claude-plugin/plugin.json": plugin_manifest_version(),
        ".claude-plugin/marketplace.json": marketplace_version(),
    }


def replace_one(path: str, pattern: str, replacement: str, *, flags: int = re.MULTILINE) -> None:
    text = read(path)
    new_text, count = re.subn(pattern, replacement, text, count=1, flags=flags)
    if count != 1:
        raise RuntimeError(f"expected exactly one version replacement in {path}, got {count}")
    write(path, new_text)


def set_version(version: str) -> None:
    if not SEMVER_RE.match(version):
        raise SystemExit(f"invalid semver: {version}")

    write("VERSION", version + "\n")
    replace_one(
        "Cargo.toml",
        r'(?ms)(^\[package\]\s+name\s*=\s*"rememora"\s+version\s*=\s*")[^"]+(")',
        rf"\g<1>{version}\2",
        flags=re.MULTILINE | re.DOTALL,
    )
    replace_one(
        "Cargo.lock",
        r'(?m)(^\[\[package\]\]\nname = "rememora"\nversion = ")[^"]+(")',
        rf"\g<1>{version}\2",
    )
    replace_one(
        "plugin/.claude-plugin/plugin.json",
        r'("version"\s*:\s*")[^"]+(")',
        rf"\g<1>{version}\2",
    )
    replace_one(
        ".claude-plugin/marketplace.json",
        r'("version"\s*:\s*")[^"]+(")',
        rf"\g<1>{version}\2",
    )


def check(tag: str | None) -> int:
    found = versions()
    expected = found["VERSION"]
    ok = True

    if not SEMVER_RE.match(expected):
        print(f"VERSION is not valid semver: {expected}", file=sys.stderr)
        ok = False

    for source, value in found.items():
        if value != expected:
            print(f"version mismatch: {source} has {value}, expected {expected}", file=sys.stderr)
            ok = False

    if tag:
        if not tag.startswith("v") or tag.startswith("plugin-v"):
            print(f"release tags must use the unified form v{expected}, got {tag}", file=sys.stderr)
            ok = False
        elif tag[1:] != expected:
            print(f"tag mismatch: {tag} does not match VERSION {expected}", file=sys.stderr)
            ok = False

    if ok:
        print(f"all version surfaces agree on {expected}")
        return 0
    return 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", nargs="?", choices=["set"], help="set all version surfaces")
    parser.add_argument("version", nargs="?", help="semver to write when using 'set'")
    parser.add_argument("--check", action="store_true", help="verify all version surfaces agree")
    parser.add_argument("--tag", help="verify a release tag, e.g. v1.2.3, matches VERSION")
    args = parser.parse_args()

    if args.check:
        return check(args.tag)
    if args.command == "set":
        if not args.version:
            parser.error("set requires a version")
        set_version(args.version)
        return check(None)
    parser.error("use --check or set <version>")


if __name__ == "__main__":
    raise SystemExit(main())
