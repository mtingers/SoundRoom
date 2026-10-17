#!/bin/zsh
set -eu
cd "${0:A:h}/.."

export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$CLANG_MODULE_CACHE_PATH"
swift build -c release --disable-sandbox --cache-path .build/cache -debug-info-format none
bin_dir=$(swift build -c release --show-bin-path --disable-sandbox --cache-path .build/cache)

app="${1:-$PWD/build/Soundroom.app}"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin_dir/Soundroom" "$app/Contents/MacOS/Soundroom"
cp "$bin_dir/SoundroomTUI" "$app/Contents/MacOS/SoundroomTUI"
cp scripts/Info.plist "$app/Contents/Info.plist"

# Remove the obsolete Swift helper when rebuilding an existing destination.
rm -rf "$app/Contents/Helpers/SoundroomServer.app"
mkdir -p "$app/Contents/Helpers"
# The portable Rust host the GUI launches by default (serve-local on loopback).
cargo build --release --manifest-path portable/Cargo.toml -p soundroom-server
cp portable/target/release/soundroom-server "$app/Contents/Helpers/soundroom-server"

if [[ -f "$PWD/web/dist/index.html" ]]; then
    mkdir -p "$app/Contents/Resources/Web"
    cp -R "$PWD/web/dist/." "$app/Contents/Resources/Web/"
fi
swift scripts/icon.swift "$PWD/build/Soundroom.iconset" "$app/Contents/Resources/Soundroom.icns"

codesign --force --sign - --identifier com.soundroom.recorder.tui "$app/Contents/MacOS/SoundroomTUI"
codesign --force --sign - --identifier com.soundroom.recorder.rust-server "$app/Contents/Helpers/soundroom-server"
codesign --force --sign - --identifier com.soundroom.recorder "$app"
print "Built $app"
