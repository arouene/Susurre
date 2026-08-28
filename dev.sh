#!/bin/sh
#
# Execute cargo in the GNOME SDK.
#
# Required on systems that does not have the gtk4 headers. On a standard
# distribution with gtk4-devel and libadwaita-devel installed, `cargo build` is
# used directly.
#
#   ./dev.sh build --release
#   ./dev.sh test
#   ./dev.sh clippy --all-targets
#   ./dev.sh run

set -eu

SDK=org.gnome.Sdk//49
LLVM=/usr/lib/sdk/llvm20
PROJECT=$(cd "$(dirname "$0")" && pwd)

# Point XDG_DATA_HOME at the installed Flatpak's data directory so a build from
# source reads the very same models as the packaged application, instead of
# looking inside the SDK's own sandbox.
APP_DATA=$HOME/.var/app/fr.arouene.Susurre/data

if ! flatpak info "$SDK" >/dev/null 2>&1; then
    echo "SDK is missing. Install with:" >&2
    echo "  flatpak install flathub $SDK org.freedesktop.Sdk.Extension.rust-stable//25.08 org.freedesktop.Sdk.Extension.llvm20//25.08" >&2
    exit 1
fi

exec flatpak run --devel --share=network --device=dri \
    --filesystem="$PROJECT" \
    --filesystem="$APP_DATA" \
    --command=sh "$SDK" -c '
        set -eu
        export PATH="/usr/lib/sdk/rust-stable/bin:'"$LLVM"'/bin:$PATH"
        export LIBCLANG_PATH="'"$LLVM"'/lib"
        export CARGO_HOME="'"$PROJECT"'/.cargo-sdk"
        export SUSURRE_CT2_HELPER="'"$PROJECT"'/python/susurre-ct2.py"
        export XDG_DATA_HOME="'"$APP_DATA"'"
        cd "'"$PROJECT"'"
        exec cargo "$@"
    ' cargo "$@"
