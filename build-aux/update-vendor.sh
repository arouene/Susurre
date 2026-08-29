#!/bin/sh
#
# Regenerate the vendored sources the Flatpak manifest builds from.
#
# Run after changing Cargo.lock or the faster-whisper version. Both outputs are
# committed, so the build itself never touches the network:
#
#   build-aux/cargo-sources.json           every crate, sha256 pinned
#   build-aux/python3-faster-whisper.yaml  every wheel, sha256 pinned
#
# The generators live in flatpak-builder-tools, which is not packaged
# anywhere useful, so they are fetched into a throwaway virtualenv.

set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

RAW=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master

# Platform wheels, not sdists, for everything that is not pure Python.
# tokenizers and hf-xet are Rust, av is C against ffmpeg, numpy needs meson and
# a BLAS: building any of them from source offline is not worth attempting.
WHEELS=onnxruntime,ctranslate2,av,hf-xet,numpy,pyyaml,tokenizers

echo "Setting up the generators in $WORK"
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/pip" -q install aiohttp toml tomlkit requirements-parser packaging PyYAML
curl -sSfLo "$WORK/cargo.py" "$RAW/cargo/flatpak-cargo-generator.py"
curl -sSfLo "$WORK/pip.py" "$RAW/pip/flatpak-pip-generator.py"

echo "Generating cargo-sources.json"
"$WORK/venv/bin/python" "$WORK/cargo.py" "$HERE/../Cargo.lock" \
    -o "$HERE/cargo-sources.json"

echo "Generating python3-faster-whisper.yaml"
"$WORK/venv/bin/python" "$WORK/pip.py" \
    --runtime=org.gnome.Sdk//49 \
    --cleanup=scripts \
    --checker-data \
    --yaml \
    --prefer-wheels="$WHEELS" \
    -o "$HERE/python3-faster-whisper" \
    faster-whisper

echo "Done. Review the diff, then rebuild with ./build-flatpak.sh"
