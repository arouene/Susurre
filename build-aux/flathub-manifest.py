#!/usr/bin/env python3
"""Derive the Flathub manifest from the one used for local builds.

Flathub has no working directory to copy from, so the `type: dir` source has to
become a git source pinned to a tag and its commit. Everything else, comments
included, is carried over untouched: keeping a second full copy of the manifest
in the Flathub repository would only let the two drift apart.

    ./build-aux/flathub-manifest.py v1.0.0 > ../flathub/fr.rouene.Susurre.yaml
"""
import pathlib
import subprocess
import sys

import yaml

URL = "https://github.com/arouene/Susurre.git"

if len(sys.argv) != 2:
    sys.exit(f"usage: {sys.argv[0]} <tag>")
tag = sys.argv[1]

here = pathlib.Path(__file__).parent
manifest = next(here.glob("*.Susurre.yaml"))
lines = manifest.read_text().splitlines(keepends=True)

# The tag has to exist locally: an unresolvable one would otherwise be pinned
# to nothing and fail only once Flathub tries to build it.
# --verify, because a bare rev-parse echoes the argument back on stdout when it
# cannot resolve it, which reads as a perfectly good answer.
rev = subprocess.run(
    ["git", "rev-parse", "--verify", f"{tag}^{{commit}}"],
    capture_output=True, text=True, cwd=here.parent,
)
if rev.returncode != 0:
    sys.exit(f"unknown tag {tag!r}; create it before generating the manifest")
commit = rev.stdout.strip()

DIR_SOURCE = "      - type: dir"
start = next(i for i, l in enumerate(lines) if l.rstrip() == DIR_SOURCE)
# The source block runs until the first line that is not indented deeper than
# the list item itself, which is where the skip list ends.
end = start + 1
while end < len(lines) and (lines[end].startswith(" " * 8) or not lines[end].strip()):
    end += 1

out = "".join(lines[:start]) + (
    f"      - type: git\n"
    f"        url: {URL}\n"
    f"        tag: {tag}\n"
    f"        commit: {commit}\n"
) + "".join(lines[end:])

parsed = yaml.safe_load(out)
sources = parsed["modules"][-1]["sources"]
assert not any(s.get("type") == "dir" for s in sources if isinstance(s, dict)), sources
assert sources[0] == {"type": "git", "url": URL, "tag": tag, "commit": commit}, sources
sys.stdout.write(out)
