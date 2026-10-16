# Soundroom release procedure

The local build is ad-hoc signed. A distribution release requires a Developer ID Application certificate/private key, hardened runtime, and Apple notarization. See [Apple's Developer ID guidance](https://developer.apple.com/developer-id/) and [notarization requirements](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution). The release entitlements allow [Core Audio input](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input); microphone use still requires the user's normal macOS consent.

## Signing prerequisites

A source build uses ad-hoc signing. A distributable release requires your own Developer ID Application identity and a Keychain notarization profile. Keep certificates, private keys, account passwords, and notarization credentials outside the repository.

## Prepare

1. Review the current tests and qualification notes, and resolve remaining release acceptance work. Capture the source commit. A successful build alone does not qualify audio behavior.
2. Use the same Xcode/Swift toolchain and macOS version when reproducing a release. Update `scripts/Info.plist` version/build first. Run from a clean checkout; preserve any local changes before creating it.
3. Set `SOUNDROOM_SIGN_IDENTITY` to the installed Developer ID Application identity, then run `zsh scripts/release-app.sh prepare`. This checkout uses `.git` in the project root; no `SOUNDROOM_GIT_DIR` override is needed. Unset any old override before preparing a release. The script requires a clean source tree, records its revision, runs release tests, builds, copies a separate release app, signs with secure timestamp/hardened runtime, verifies the signature, and creates a ZIP plus SHA-256 and toolchain manifest under `build/Release`.
4. Review the app and test/build logs. Launch that exact signed copy through Finder. Check input/output routing, microphone consent, a built-in-device take, pause/resume/seek, preview and both designers, save/reopen, clip history and exported WAV. Keep hardware qualification results separate for interfaces/controllers that were unavailable.

## Notarize

Use an existing `notarytool` Keychain profile named in `SOUNDROOM_NOTARY_PROFILE`; configure it locally using Apple's instructions. Run `zsh scripts/release-app.sh notarize` to build/sign, submit to Apple, require an Accepted result, staple/validate the ticket, assess with Gatekeeper, and regenerate the ZIP/checksum. This command uploads the app to Apple's notarization service. Re-run the native smoke check on the exact stapled app, then distribute that ZIP and checksum together.

A notarization rejection is recorded in `notarization.json`. Obtain its detailed log with `xcrun notarytool log <submission-id> --keychain-profile <profile>`. Correct the reported problem before retrying. Keep the last accepted artifact until its replacement passes all checks.

## Reproducibility limits

The procedure records source revision, compiler/Xcode/macOS versions, the ad-hoc development binary hash, and the distribution binary/archive hashes. It is repeatable from the recorded commit. Signed/notarized ZIPs are not expected to be byte-identical across runs because signatures and notarization tickets contain service-generated metadata. Retain the manifest and exact source revision for comparisons.
