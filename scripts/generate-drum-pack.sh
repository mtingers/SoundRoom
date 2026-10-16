#!/bin/zsh
set -eu
cd "${0:A:h}/.."
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$CLANG_MODULE_CACHE_PATH"
swift build -c release --product DrumPackGenerator --disable-sandbox --cache-path .build/cache -debug-info-format none
bin_dir=$(swift build -c release --show-bin-path --disable-sandbox --cache-path .build/cache)
destination="${1:-$PWD/build/DrumPacks}"
"$bin_dir/DrumPackGenerator" "$destination"
python3 scripts/validate-drum-pack.py "$destination"
