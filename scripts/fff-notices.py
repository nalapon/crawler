#!/usr/bin/env python3
"""Collect notices for the statically linked dependencies selected by Cargo."""
import pathlib
import re
import subprocess
import sys
import tomllib

root = pathlib.Path(sys.argv[1])
toolchain = sys.argv[2]
tree = subprocess.check_output([
    "rustup", "run", toolchain, "cargo", "tree", "--manifest-path", str(root / "Cargo.toml"),
    "--locked", "-p", "fff-search", "--edges", "normal", "--prefix", "none", "--format", "{p}",
], text=True)
packages = {match.groups() for line in tree.splitlines()
            if (match := re.match(r"([\w-]+) v([^ ]+)", line))}
registry = list((pathlib.Path.home() / ".cargo/registry/src").glob("*"))
notices = ["FFF is statically linked. Versions come from upstream Cargo.lock.\n",
           (root / "LICENSE").read_text()]
for name, version in sorted(packages):
    if name.startswith("fff-"):
        continue  # Workspace crates share the upstream MIT notice above.
    directory = next((parent / f"{name}-{version}" for parent in registry
                      if (parent / f"{name}-{version}").is_dir()), None)
    if directory is None:
        raise RuntimeError(f"Missing dependency sources for {name} {version}")
    manifest = tomllib.loads((directory / "Cargo.toml").read_text())
    notices.append(f"\n\n{name} {version}\nhttps://crates.io/crates/{name}/{version}\n"
                   f"License: {manifest['package'].get('license', 'see upstream source')}\n")
    licenses = set(directory.glob("LICENSE*")) | set(directory.glob("COPYING*")) | set(directory.glob("UNLICENSE*"))
    for license in sorted(licenses):
        if license.is_file():
            notices.append(license.read_text(errors="replace"))
(root.parent / "THIRD_PARTY.txt").write_text("\n".join(notices))
