#!/usr/bin/env python3
"""Collect license notices for every crate statically linked into the Crawler executable.

Usage: rust-notices.py <crate dir> <output file>
"""
import json
import pathlib
import subprocess
import sys

crate, output = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked",
     "--filter-platform", "aarch64-apple-darwin"],
    cwd=crate, text=True))
packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}

# Walk normal (non-dev, non-build) dependencies from the root package.
linked, pending = set(), [metadata["resolve"]["root"]]
while pending:
    node = nodes[pending.pop()]
    for dep in node["deps"]:
        if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in linked:
            linked.add(dep["pkg"])
            pending.append(dep["pkg"])

def license_files(manifest: pathlib.Path) -> list[pathlib.Path]:
    """License files beside the crate, or in a parent folder for workspace members."""
    folder = manifest.parent
    for candidate in [folder, *folder.parents][:4]:
        found = sorted(f for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*")
                       for f in candidate.glob(pattern) if f.is_file())
        if found:
            return found
    return []

notices = ["Crawler statically links the following Rust crates.\n"]
missing = []
for package in sorted((packages[i] for i in linked), key=lambda p: (p["name"], p["version"])):
    files = license_files(pathlib.Path(package["manifest_path"]))
    notices.append(f"\n\n{'=' * 72}\n{package['name']} {package['version']}\n"
                   f"License: {package.get('license') or 'see license text'}\n")
    if not files:
        missing.append(package["name"])
    for file in files:
        notices.append(f"\n--- {file.name} ---\n{file.read_text(errors='replace')}")
output.write_text("".join(notices))
print(f"{len(linked)} crates, {len(missing)} without a license file"
      + (f": {', '.join(missing)}" if missing else ""))
