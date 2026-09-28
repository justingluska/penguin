#!/bin/sh
# The app process on a Mac, measured: launch milestones from penguin.log,
# memory footprint, threads, and CPU and idle wakeups while Penguin sits in
# the background. What the numbers mean and the Linux ones to compare with:
# docs/PERFORMANCE.md, "The app process".
#
#   scripts/process-stats.sh          # launch lines, memory, threads, 5 idle minutes
#   scripts/process-stats.sh 60       # a shorter idle window
#   sudo scripts/process-stats.sh     # also per-app energy (powermetrics)
#
# Quit and reopen Penguin first for fresh launch lines, wait until the sync
# status says it's up to date, then run this and leave the Mac alone. Then
# again after a search (the model loads) and after 11 idle minutes (it
# unloads); PERFORMANCE.md lists the three runs.
set -eu
[ "$(uname)" = Darwin ] || { echo "process-stats: run this on the Mac" >&2; exit 1; }
secs="${1:-300}"
pid="$(pgrep -n -f 'Penguin.app/Contents/MacOS/' || true)"
[ -n "$pid" ] || { echo "process-stats: Penguin isn't running" >&2; exit 1; }
# Under sudo, the log is still the user's.
home="$(eval echo "~${SUDO_USER:-$(id -un)}")"
log="$home/Library/Logs/co.gluska.penguin/penguin.log"

echo "== Last launch (ms since start)"
grep -E 'launch ms=|semantic index opened|semantic model files checked|semantic model loaded|released free heap' "$log" | tail -10 | sed 's/^[^ ]* *//'

echo "== Memory"
# Physical footprint is what Activity Monitor's Memory column shows.
vmmap --summary "$pid" 2>/dev/null | grep -E 'Physical footprint' || true
ps -o rss= -p "$pid" | awk '{ printf "RSS: %.0f MB\n", $1 / 1024 }'
echo "Threads: $(ps -M -p "$pid" | tail -n +2 | wc -l | tr -d ' ')"

echo "== Idle for ${secs}s (don't touch Penguin)"
# Delta mode (-c d): the second sample counts only the interval. IDLEW is
# "package idle exit" wakeups; POWER is Activity Monitor's energy impact.
top -l 2 -s "$secs" -c d -pid "$pid" -stats pid,cpu,idlew,power,threads | tail -1 |
  awk -v s="$secs" '{ printf "CPU %s%%  idle wakeups %s (%.1f/min)  energy impact %s  threads %s\n", $2, $3, $3 * 60 / s, $4, $5 }'

if [ "$(id -u)" = 0 ]; then
  # Grouped by coalition, so WKWebView's web-content and networking
  # processes count with Penguin.
  echo "== powermetrics, 60 s (Penguin's coalition)"
  powermetrics --samplers tasks --show-process-coalition --show-process-energy -i 60000 -n 1 2>/dev/null |
    awk '/^Name/ && !h { print; h = 1 } /Penguin|co[.]gluska[.]penguin/ && !n { n = 6 } n > 0 { print; n-- }'
fi
