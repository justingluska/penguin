#!/usr/bin/env bash
# Penguin's benchmark suite, end to end:
#   1. backend: crates/penguin-core/examples/bench.rs (release) on synthetic
#      mailboxes of 10k, 100k and 300k messages, one after another;
#   2. UI: scripts/bench-ui (the mock build in headless Chromium);
#   3. report: docs/PERFORMANCE.md and the Performance block in README.md,
#      from every results file in docs/perf/.
#
#   scripts/bench.sh                    # everything
#   scripts/bench.sh --corpus 100k      # one corpus (repeatable)
#   scripts/bench.sh --no-ui            # backend only
#   scripts/bench.sh --ui-only          # UI only
#   scripts/bench.sh --report-only      # regenerate the docs from docs/perf/*.json
#   scripts/bench.sh --quick            # fewer repetitions (smoke run; don't publish)
#
# Results land in docs/perf/<os>-<arch>-*.json, so a run on a Mac adds its
# own files next to the Linux ones instead of replacing them.
#
# Disk: the 300k corpus needs about 3 GB while it runs (deleted afterwards).
# The database goes to target/bench-corpus, or $PENGUIN_BENCH_DIR.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

corpora=()
ui=1
backend=1
report=1
quick=()  # expanded as ${quick[@]+...}: bash 3.2 (macOS) treats an empty array as unbound under set -u
while [[ $# -gt 0 ]]; do
  case "$1" in
    --corpus) corpora+=("$2"); shift 2 ;;
    --no-ui) ui=0; shift ;;
    --ui-only) backend=0; shift ;;
    --report-only) backend=0; ui=0; shift ;;
    --quick) quick=(--quick); shift ;;
    -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
    *) echo "bench.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[[ ${#corpora[@]} -eq 0 ]] && corpora=(10k 100k 300k)

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) os="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
esac
arch="$(uname -m)"
[[ "$arch" == arm64 ]] && arch=aarch64
tag="$os-$arch"
out="$root/docs/perf"
mkdir -p "$out"
db_dir="${PENGUIN_BENCH_DIR:-$root/target/bench-corpus}"

# On the shared Linux build box, cargo goes through box-cargo.sh (it caps
# concurrent builds); elsewhere plain cargo.
cargo_cmd=(cargo)
if [[ -n "${PENGUIN_CARGO:-}" ]]; then
  read -r -a cargo_cmd <<<"$PENGUIN_CARGO"
elif [[ "$os" == linux && -x "$root/scripts/box-cargo.sh" && -d "$HOME/.local/tauri-sysroot" ]]; then
  cargo_cmd=("$root/scripts/box-cargo.sh")
fi

if [[ $backend -eq 1 ]]; then
  "${cargo_cmd[@]}" build --release -p penguin-core --example bench
  bin="$root/target/release/examples/bench"
  for c in "${corpora[@]}"; do
    echo "== backend, $c messages" >&2
    "$bin" --corpus "$c" --dir "$db_dir" --out "$out/$tag-backend-$c.json" ${quick[@]+"${quick[@]}"}
  done
fi

if [[ $ui -eq 1 ]]; then
  echo "== UI (headless Chromium)" >&2
  if [[ ! -d "$root/scripts/bench-ui/node_modules" ]]; then
    (cd "$root/scripts/bench-ui" && npm ci --no-audit --no-fund)
  fi
  node "$root/scripts/bench-ui/bench.mjs" --out "$out/$tag-ui.json" ${quick[@]+"${quick[@]}"}
fi

if [[ $report -eq 1 ]]; then
  node "$root/scripts/bench-report.mjs"
fi
