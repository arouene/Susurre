#!/bin/sh
# Build and install the Flatpak package.
#
#   ./build-flatpak.sh            build and install for the current user
#   ./build-flatpak.sh --bundle   also produce susurre.flatpak, distributable
#   ./build-flatpak.sh --clean    start from an empty cache
#   ./build-flatpak.sh --flathub  dry run the submission, does not install

set -eu

APP=fr.rouene.Susurre
MANIFEST=build-aux/$APP.yaml
cd "$(dirname "$0")"

bundle=false
flathub=
for arg in "$@"; do
    case "$arg" in
        --bundle)
            bundle=true
            ;;
        --flathub)
            flathub=$(git describe --tags --abbrev=0)
            ;;
        --flathub=*)
            flathub=${arg#*=}
            ;;
        --clean)
            rm -rf .flatpak-builder build repo .flathub-rehearsal
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

# Rehearses the Flathub submission rather than a local install.
#
# Two things differ from the build below and both have caught real problems:
# the manifest is the generated one, whose git source is the only kind Flathub
# can fetch, and flathub-build passes --sandbox, which drops the build-args a
# local build silently allows. A module that still wanted the network would
# pass here and fail only in review.
#
# Linter findings are printed but do not fail the run: an error can be a
# legitimate exception to request in the submission, as the IBus paths are.
if [ -n "$flathub" ]; then
    # A fixed directory rather than a mktemp one: flatpak-builder keeps its
    # download cache and its ccache beside the manifest, and a throwaway
    # directory made every rehearsal refetch all 317 crates and recompile
    # whisper.cpp from scratch. Purged by --clean.
    work=$PWD/.flathub-rehearsal
    mkdir -p "$work"
    cp build-aux/cargo-sources.json build-aux/python3-faster-whisper.yaml "$work/"
    ./build-aux/flathub-manifest.py "$flathub" > "$work/$APP.yaml"
    echo "Rehearsing $APP at $flathub in $work"

    ( cd "$work" && flatpak run --filesystem="$work" \
        --command=flathub-build org.flatpak.Builder "$APP.yaml" )

    for target in "manifest $work/$APP.yaml" "repo $work/repo"; do
        # shellcheck disable=SC2086
        set -- $target
        echo "== flatpak-builder-lint $1"
        flatpak run --filesystem="$work" \
            --command=flatpak-builder-lint org.flatpak.Builder "$1" "$2" || true
    done
    exit 0
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
