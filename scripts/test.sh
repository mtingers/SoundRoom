#!/bin/zsh
set -eu
cd "${0:A:h}/.."
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$CLANG_MODULE_CACHE_PATH"
swift test --disable-sandbox --cache-path .build/cache
