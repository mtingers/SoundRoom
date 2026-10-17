#!/bin/zsh
set -eu
cd "${0:A:h}/.."
release_mode=${1:-prepare}
if [[ "$release_mode" != prepare && "$release_mode" != notarize ]]; then
    print -u2 'Usage: zsh scripts/release-app.sh [prepare|notarize]'
    exit 2
fi
: "${SOUNDROOM_SIGN_IDENTITY:?Set SOUNDROOM_SIGN_IDENTITY to a Developer ID Application identity}"
if [[ "$SOUNDROOM_SIGN_IDENTITY" != 'Developer ID Application:'* ]]; then
    print -u2 'Distribution requires a Developer ID Application identity.'
    exit 2
fi
if [[ "$release_mode" == notarize ]]; then
    : "${SOUNDROOM_NOTARY_PROFILE:?Set SOUNDROOM_NOTARY_PROFILE to an existing notarytool Keychain profile}"
fi
release_git=(git)
if [[ -n "${SOUNDROOM_GIT_DIR:-}" ]]; then
    release_git+=(--git-dir "$SOUNDROOM_GIT_DIR" --work-tree "$PWD")
fi
release_revision=$("${release_git[@]}" rev-parse HEAD)
if [[ -n "$("${release_git[@]}" status --porcelain)" ]]; then
    print -u2 'Commit or preserve local changes before preparing a distribution release.'
    exit 2
fi
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$CLANG_MODULE_CACHE_PATH"
mkdir -p build/Release
swift test -c release --disable-sandbox --cache-path .build/cache -debug-info-format none \
    > build/Release/tests.log 2>&1
zsh scripts/build-app.sh > build/Release/build.log 2>&1
release_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' scripts/Info.plist)
release_folder="$PWD/build/Release/Soundroom-$release_version"
mkdir -p "$release_folder"
ditto build/Soundroom.app "$release_folder/Soundroom.app"
release_app="$release_folder/Soundroom.app"
codesign --force --sign "$SOUNDROOM_SIGN_IDENTITY" --timestamp --options runtime \
    --entitlements scripts/Release.entitlements "$release_app"
codesign --verify --deep --strict --verbose=2 "$release_app"
release_archive="$release_folder/Soundroom-$release_version.zip"
ditto -c -k --keepParent "$release_app" "$release_archive"
{
    print "Source revision: $release_revision"
    swift --version
    xcodebuild -version
    sw_vers
    shasum -a 256 build/Soundroom.app/Contents/MacOS/Soundroom
    shasum -a 256 "$release_app/Contents/MacOS/Soundroom"
    shasum -a 256 scripts/Info.plist scripts/Release.entitlements
} > "$release_folder/build-manifest.txt"
if [[ "$release_mode" == notarize ]]; then
    xcrun notarytool submit "$release_archive" --keychain-profile "$SOUNDROOM_NOTARY_PROFILE" --wait --output-format json > "$release_folder/notarization.json"
    python3 - "$release_folder/notarization.json" <<'PY'
import json, sys

with open(sys.argv[1]) as source:
    result = json.load(source)
if result.get("status") != "Accepted":
    sys.exit(
        "Notarization was not accepted; inspect notarization.json and obtain the notarytool log."
    )
PY
    xcrun stapler staple "$release_app"
    xcrun stapler validate "$release_app"
    spctl --assess --type execute --verbose=2 "$release_app"
    ditto -c -k --keepParent "$release_app" "$release_archive"
fi
shasum -a 256 "$release_archive" > "$release_archive.sha256"
print "Prepared $release_archive ($release_mode)"
