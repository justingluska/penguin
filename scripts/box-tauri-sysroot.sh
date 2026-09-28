#!/usr/bin/env bash
# Build a rootless sysroot with the GTK/WebKit dev packages the Tauri desktop
# crate needs, for Linux machines where we have no sudo (a shared dev box).
#
#   scripts/box-tauri-sysroot.sh          # idempotent: download + extract + fix up
#   source scripts/box-tauri-env.sh       # then use cargo as usual
#
# How it works: `apt-get install -s` (simulation, no root) resolves exactly the
# packages that are missing on this machine; `apt-get download` fetches them;
# `dpkg-deb -x` unpacks them into $TAURI_SYSROOT. The .pc files are rewritten so
# their prefixes point into the sysroot, and dev symlinks (libfoo.so) whose
# runtime library is installed system-wide rather than in the sysroot are
# re-pointed at the system copy.
set -euo pipefail

ROOT="${TAURI_SYSROOT:-$HOME/.local/tauri-sysroot}"
DEBS="${TAURI_SYSROOT_DEBS:-$HOME/.cache/tauri-sysroot-debs}"
MULTIARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)"

WANT=(
  pkgconf
  libwebkit2gtk-4.1-dev
  libjavascriptcoregtk-4.1-dev
  libsoup-3.0-dev
  libgtk-3-dev
  libayatana-appindicator3-dev
  librsvg2-dev
  libssl-dev
)

mkdir -p "$ROOT" "$DEBS"

# 1. Resolve the missing set (only packages not installed system-wide).
mapfile -t PKGS < <(
  apt-get install -s --no-install-recommends "${WANT[@]}" 2>/dev/null |
    awk '/^Inst /{print $2}'
)
echo "box-tauri-sysroot: ${#PKGS[@]} packages not installed system-wide"

# 2. Download the ones we don't have yet (apt-get download writes to cwd).
need=()
for p in "${PKGS[@]}"; do
  compgen -G "$DEBS/${p}_*.deb" >/dev/null || need+=("$p")
done
if ((${#need[@]})); then
  echo "box-tauri-sysroot: downloading ${#need[@]} .deb files"
  (cd "$DEBS" && apt-get download "${need[@]}")
fi

# 3. Extract each .deb once (stamp per file so re-runs are cheap).
STAMPS="$ROOT/.extracted"
mkdir -p "$STAMPS"
for p in "${PKGS[@]}"; do
  for deb in "$DEBS/${p}"_*.deb; do
    stamp="$STAMPS/$(basename "$deb").ok"
    [[ -e "$stamp" ]] && continue
    dpkg-deb -x "$deb" "$ROOT"
    touch "$stamp"
  done
done

# 4. Rewrite pkg-config prefixes into the sysroot.
for dir in "$ROOT/usr/lib/$MULTIARCH/pkgconfig" "$ROOT/usr/share/pkgconfig" "$ROOT/usr/lib/pkgconfig"; do
  [[ -d "$dir" ]] || continue
  for pc in "$dir"/*.pc; do
    [[ -e "$pc" ]] || continue
    # Only rewrite lines that still point at /usr (keeps this idempotent).
    sed -i -E \
      -e "s#^([A-Za-z_]+)=/usr(/|\$)#\1=$ROOT/usr\2#" \
      -e "s#(-[IL])/usr/#\1$ROOT/usr/#g" \
      "$pc"
  done
done

# 5. Re-point dangling dev symlinks at the system runtime library.
#    e.g. sysroot/usr/lib/<arch>/libgtk-3.so -> libgtk-3.so.0 (only in /usr/lib).
libdir="$ROOT/usr/lib/$MULTIARCH"
#    Subdirectories first (mit-krb5/libkrb5.so -> ../libkrb5.so.3.3), so the
#    top-level links that chain through them (libkrb5.so -> mit-krb5/...) resolve.
fix_links() { # find depth args...
  while IFS= read -r -d '' link; do
    [[ -e "$link" ]] && continue
    target="$(readlink "$link")"
    base="$(basename "$target")"
    for sys in "/usr/lib/$MULTIARCH/$base" "/lib/$MULTIARCH/$base"; do
      if [[ -e "$sys" ]]; then ln -sfn "$sys" "$link"; break; fi
    done
    [[ -e "$link" ]] || echo "box-tauri-sysroot: warning: dangling $link -> $target" >&2
  done < <(find "$libdir" "$@" -type l -print0)
}
if [[ -d "$libdir" ]]; then
  fix_links -mindepth 2 -maxdepth 2
  fix_links -mindepth 1 -maxdepth 1
fi

echo "box-tauri-sysroot: ready at $ROOT (now: source scripts/box-tauri-env.sh)"
