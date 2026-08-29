#!/bin/sh
# Build and install the Flatpak package.
#
#   ./build-flatpak.sh            build and install for the current user
#   ./build-flatpak.sh --bundle   also produce susurre.flatpak, distributable
#   ./build-flatpak.sh --clean    start from an empty cache

set -eu

APP=fr.arouene.Susurre
MANIFEST=build-aux/$APP.yaml
cd "$(dirname "$0")"

bundle=false
for arg in "$@"; do
    case "$arg" in
        --bundle)
            bundle=true
            ;;
        --clean)
            rm -rf .flatpak-builder build repo
            ;;
        *)
            echo "unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

# The Flatpak build is offline, so a dependency change that has not been
# regenerated would fail deep inside cargo with no hint about the generator.
# Development does not go through here at all: ./dev.sh runs cargo online
# against the host registry and ignores the vendored tree entirely.
./build-aux/check-vendor.py

# Always use org.flatpak.Builder, this is what Flathub recommends.
if ! flatpak info org.flatpak.Builder >/dev/null 2>&1; then
    echo "Installing org.flatpak.Builder..." >&2
    flatpak install -y --user flathub org.flatpak.Builder
fi

build() {
    flatpak run org.flatpak.Builder --user --install --force-clean \
        --install-deps-from=flathub --repo=repo build "$MANIFEST"
}

# A running instance would lock the deployment.
flatpak kill "$APP" 2>/dev/null || true

# The flatpak-builder cache can gets corrupt when a build is interrupted, or
# when it was copied from another machine: "Failed to check out cache". Purging
# is enough to recover.
if ! build; then
    echo "Build failed. Purging the cache and retrying once..." >&2
    rm -rf .flatpak-builder build
    build
fi

if [ "$bundle" = true ]; then
    flatpak build-bundle repo susurre.flatpak "$APP"
    echo "Bundle written: $(pwd)/susurre.flatpak"
fi

echo "Installed. Run with: flatpak run $APP"
