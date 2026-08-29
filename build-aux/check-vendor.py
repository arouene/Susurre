#!/usr/bin/env python3
"""Fail when the vendored cargo sources no longer match Cargo.lock.

The offline build would otherwise stop on a cargo error that says a crate is
missing from the vendor directory, without ever mentioning the generator that
puts it there.
"""
import json
import pathlib
import re
import sys

here = pathlib.Path(__file__).parent

lock = here.parent / "Cargo.lock"
locked = {
    f"{name}-{version}"
    for name, version in re.findall(
        r'^name = "(.+)"\nversion = "(.+)"$', lock.read_text(), re.M
    )
}

# The crate being built has no source of its own and is never vendored. Read it
# from Cargo.toml rather than hardcoding it, so a version bump does not turn
# into a spurious failure on the day of a release.
manifest = (here.parent / "Cargo.toml").read_text()
root = re.search(r'^name = "(.+)"$', manifest, re.M)
version = re.search(r'^version = "(.+)"$', manifest, re.M)
if not root or not version:
    sys.exit("cannot read the package name and version from Cargo.toml")
locked -= {f"{root[1]}-{version[1]}"}

sources = json.loads((here / "cargo-sources.json").read_text())
vendored = {
    s["dest"].removeprefix("cargo/vendor/")
    for s in sources
    if s["type"] == "archive"
}

missing = sorted(locked - vendored)
extra = sorted(vendored - locked)
if not missing and not extra:
    sys.exit(0)

print("build-aux/cargo-sources.json is out of date with Cargo.lock:", file=sys.stderr)
for crate in missing:
    print(f"  missing: {crate}", file=sys.stderr)
for crate in extra:
    print(f"  stale:   {crate}", file=sys.stderr)
print("\nRegenerate it:\n  ./build-aux/update-vendor.sh", file=sys.stderr)
sys.exit(1)
