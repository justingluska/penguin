#!/usr/bin/env bash
# Run cargo on a shared, RAM-limited Linux build box without starving it: at
# most two cargo runs box-wide (two flock slots) and -j 2. Each checkout keeps
# its own target dir: worktrees sharing one overwrite each other's crates
# (same names, different sources) and fail with missing items.
#   scripts/box-cargo.sh test -p penguin-desktop
# Many parallel worktree builds can exhaust RAM; each running slot costs ~2-3 GB.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
source "$here/scripts/box-tauri-env.sh"
export CARGO_BUILD_JOBS=2
locks="$HOME/.cache/penguin-cargo-locks"
mkdir -p "$locks"

run_in_slot() { # slot args...
  local slot="$1"
  shift
  echo "box-cargo: slot $slot" >&2
  cargo "$@"
}

# Take a free slot if there is one, else wait for slot 0.
for slot in 0 1; do
  exec {fd}>"$locks/slot-$slot.lock"
  if flock -n "$fd"; then
    run_in_slot "$slot" "$@"
    exit $?
  fi
  exec {fd}>&-
done
echo "box-cargo: both slots busy, waiting…" >&2
exec {fd}>"$locks/slot-0.lock"
flock "$fd"
run_in_slot 0 "$@"
