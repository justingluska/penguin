# Source me: point cargo/pkg-config at the rootless Tauri sysroot built by
# scripts/box-tauri-sysroot.sh (run that first; it is idempotent).
#
#   source scripts/box-tauri-env.sh
#   cargo check -p penguin-desktop -j 4
#   cargo test  -p penguin-desktop -j 4 settings
#
# shellcheck shell=bash

_tr_root="${TAURI_SYSROOT:-$HOME/.local/tauri-sysroot}"
_tr_arch="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)"
_tr_lib="$_tr_root/usr/lib/$_tr_arch"

if [[ ! -x "$_tr_root/usr/bin/pkgconf" ]]; then
  echo "box-tauri-env: no sysroot at $_tr_root; run scripts/box-tauri-sysroot.sh first" >&2
  unset _tr_root _tr_arch _tr_lib
  return 1 2>/dev/null || exit 1
fi

[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

_tr_prepend() { # var value
  local cur="${!1:-}"
  case ":$cur:" in *":$2:"*) ;; *) export "$1=$2${cur:+:$cur}" ;; esac
}

export TAURI_SYSROOT="$_tr_root"
# pkgconf itself lives in the sysroot and needs its own libpkgconf.
export PKG_CONFIG="$_tr_root/usr/bin/pkgconf"
# System .pc dirs are pkgconf's compiled-in defaults and are searched after these.
_tr_prepend PKG_CONFIG_PATH "$_tr_root/usr/share/pkgconfig"
_tr_prepend PKG_CONFIG_PATH "$_tr_lib/pkgconfig"
_tr_prepend PATH "$_tr_root/usr/bin"
_tr_prepend LIBRARY_PATH "$_tr_lib"
_tr_prepend LD_LIBRARY_PATH "$_tr_lib"
_tr_prepend C_INCLUDE_PATH "$_tr_root/usr/include"
_tr_prepend CPLUS_INCLUDE_PATH "$_tr_root/usr/include"
# GSettings schemas / GIO modules shipped in the sysroot (runtime only).
export XDG_DATA_DIRS="${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
_tr_prepend XDG_DATA_DIRS "$_tr_root/usr/share"
[[ -d "$_tr_lib/gio/modules" ]] && export GIO_EXTRA_MODULES="$_tr_lib/gio/modules"

# /tmp is noexec on this box; tests that write and exec a script under
# std::env::temp_dir() (rules::hooks) need an exec-capable TMPDIR.
if findmnt -no OPTIONS -T "${TMPDIR:-/tmp}" 2>/dev/null | grep -qw noexec; then
  export TMPDIR="$HOME/.cache/penguin-tmp"
  mkdir -p "$TMPDIR"
fi

unset -f _tr_prepend
unset _tr_root _tr_arch _tr_lib
