#!/bin/sh
# Build Penguin from the latest main and install it over /Applications/Penguin.app.
# Run on the Mac, from anywhere inside the repo:
#
#   scripts/install-local.sh            # pull main, build, install, relaunch
#   scripts/install-local.sh --no-pull  # build what's checked out
#
# Signs with the same Apple Development identity as the current install, so
# the Keychain grant for Penguin's token item keeps working (no re-sign-in).
# Identity: $APPLE_SIGNING_IDENTITY, else the first "Apple Development"
# identity in the login keychain. Takes 3–5 min on a warm cache.
set -eu

warn() { printf 'install-local: %s\n' "$*" >&2; }
die() { warn "$*"; exit 1; }

[ "$(uname)" = Darwin ] || die "this builds the macOS app; run it on the Mac"
root="$(git rev-parse --show-toplevel)"
cd "$root"

if [ "${1:-}" != "--no-pull" ]; then
  [ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes here; commit or stash them, or pass --no-pull"
  git checkout -q main
  git pull -q --ff-only origin main
fi
printf 'Building %s\n' "$(git log -1 --format='%h %s')"

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null | awk '/"Apple Development/ { print $2; exit }')"
fi
[ -n "$identity" ] || die "no Apple Development signing identity in the login keychain (set APPLE_SIGNING_IDENTITY)"

cd apps/desktop
# Reinstall JS deps only when the lockfile changed since the last install.
stamp=node_modules/.install-local-lock
if [ ! -f "$stamp" ] || ! cmp -s package-lock.json "$stamp"; then
  npm ci
  cp package-lock.json "$stamp"
fi

APPLE_SIGNING_IDENTITY="$identity" npm run tauri build -- --bundles app

app=""
for dir in "$root/target/release/bundle/macos" src-tauri/target/release/bundle/macos; do
  [ -d "$dir/Penguin.app" ] && app="$dir/Penguin.app" && break
done
[ -n "$app" ] || die "build finished but Penguin.app wasn't found under target/release/bundle/macos"

# Quit the running copy, keep the old one in the Trash (not rm), then install.
osascript -e 'tell application id "co.gluska.penguin" to quit' 2>/dev/null || true
for _ in 1 2 3 4 5 6 7 8 9 10; do
  pgrep -xq Penguin || break
  sleep 0.5
done
if [ -d /Applications/Penguin.app ]; then
  osascript -e 'tell application "Finder" to delete POSIX file "/Applications/Penguin.app"' >/dev/null
fi
ditto "$app" /Applications/Penguin.app
open /Applications/Penguin.app
printf 'Installed %s\n' "$(git log -1 --format='%h %s')"
