#!/bin/sh
# Cargo runner for macOS dev builds (wired up in .cargo/config.toml).
#
# Every rebuild gives an ad-hoc-signed binary a new code identity, so macOS
# Keychain "Always Allow" grants never stick and Penguin re-prompts for its
# token item. Signing the app binaries with a stable Apple Development
# identity and a fixed identifier keeps the Keychain ACL valid across rebuilds.
#
# Identity: $PENGUIN_DEV_SIGN_IDENTITY (SHA-1 hash or name), else the first
# "Apple Development" identity in the login keychain, else run unsigned.
# Only penguin-desktop and penguin-cli are signed; test binaries and anything
# else cargo runs are exec'd untouched.

set -u
bin="$1"
shift

warn() { printf 'dev-codesign-run: %s\n' "$*" >&2; }

case "$(basename "$bin")" in
  penguin-desktop) identifier="co.gluska.penguin.dev" ;;
  penguin-cli) identifier="co.gluska.penguin.cli" ;;
  *) exec "$bin" "$@" ;;
esac

identity="${PENGUIN_DEV_SIGN_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null |
    awk '/"Apple Development/ { print $2; exit }')"
fi
if [ -z "$identity" ]; then
  warn "no Apple Development signing identity; running unsigned (Keychain will re-prompt after rebuilds)"
  exec "$bin" "$@"
fi

# Always re-sign: cargo re-copies the linker's ad-hoc-signed binary from
# target/*/deps on every run, so the previous signature never survives.
if ! out="$(codesign --force --sign "$identity" --identifier "$identifier" --timestamp=none "$bin" 2>&1)"; then
  warn "codesign failed; running unsigned: $out"
fi
exec "$bin" "$@"
