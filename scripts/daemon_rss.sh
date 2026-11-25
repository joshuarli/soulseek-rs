#!/usr/bin/env bash
# Reproducible daemon RSS envelope under a mixed local Soulseek load.
#
#   scripts/daemon_rss.sh
#   DAEMON_RSS_ROUNDS=16 scripts/daemon_rss.sh
#
# Starts a local soulfind, a seeder daemon, and the measured daemon, then drives
# concurrent searches, browses and downloads in both directions. A sampler
# records the measured daemon's RSS and thread count; the result is a machine
# readable JSON file containing the git revision, the exact load, the peak RSS
# and the measurement boundaries.
set -euo pipefail

cd "$(dirname "$0")/.."

SOULFIND_BIN="${SOULFIND_BIN:-$PWD/.work/tmp/soulfind/bin/soulfind}"
BIN="${SOULSEEK_BIN:-$PWD/target/debug/soulseek-rs}"
WORK="${DAEMON_RSS_WORK:-$PWD/.work/tmp/daemon-rss}"
OUT="${DAEMON_RSS_OUT:-$WORK/daemon-rss.json}"
ROUNDS="${DAEMON_RSS_ROUNDS:-8}"

if [ ! -x "$SOULFIND_BIN" ]; then
  echo "daemon-rss: no soulfind at $SOULFIND_BIN — set SOULFIND_BIN" >&2
  exit 2
fi
if [ ! -x "$BIN" ]; then
  echo "daemon-rss: no client at $BIN — set SOULSEEK_BIN" >&2
  exit 2
fi

read -r SERVER_PORT SEED_PORT LOAD_PORT SEED_LISTEN LOAD_LISTEN <<EOF
$(python3 - <<'PY'
import socket

def port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

print(*[port() for _ in range(5)])
PY
)
EOF

rm -rf "$WORK"
mkdir -p "$WORK/seed-share" "$WORK/load-share" "$WORK/seed-state" "$WORK/load-state" \
         "$WORK/seed-downloads" "$WORK/load-downloads"
printf 'daemon-rss-fixture-0123456789' > "$WORK/seed-share/rss-file.bin"
printf 'daemon-rss-fixture-0123456789' > "$WORK/load-share/rss-file.bin"

pids=""
cleanup() {
  for pid in $pids; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT

"$SOULFIND_BIN" -p "$SERVER_PORT" -d "$WORK/soulfind.db" > "$WORK/soulfind.log" 2>&1 &
pids="$pids $!"
sleep 1

SOULSEEK_STATE_DIR="$WORK/seed-state" "$BIN" --no-config --quiet \
  --server "127.0.0.1:$SERVER_PORT" --username rssseed --password pw \
  --listener-port "$SEED_LISTEN" --shared-dir "$WORK/seed-share" \
  --download-dir "$WORK/seed-downloads" daemon > "$WORK/seed.log" 2>&1 &
pids="$pids $!"

SOULSEEK_STATE_DIR="$WORK/load-state" "$BIN" --no-config --quiet \
  --server "127.0.0.1:$SERVER_PORT" --username rssload --password pw \
  --listener-port "$LOAD_LISTEN" --shared-dir "$WORK/load-share" \
  --download-dir "$WORK/load-downloads" daemon > "$WORK/load.log" 2>&1 &
LOAD_PID=$!
pids="$pids $LOAD_PID"
sleep 3

if ! kill -0 "$LOAD_PID" 2>/dev/null; then
  echo "daemon-rss: measured daemon exited" >&2
  exit 3
fi

(
  while kill -0 "$LOAD_PID" 2>/dev/null; do
    rss=$(ps -o rss= -p "$LOAD_PID" 2>/dev/null || true)
    if [ -r "/proc/$LOAD_PID/status" ]; then
      threads=$(awk '/^Threads:/{print $2}' "/proc/$LOAD_PID/status")
    else
      threads=$(ps -M -p "$LOAD_PID" 2>/dev/null | wc -l | tr -d ' ')
    fi
    printf '%s %s\n' "${rss:-0}" "${threads:-0}"
    sleep 0.1
  done
) > "$WORK/samples.txt" &
pids="$pids $!"

LOAD_SOCK="$WORK/load-state/daemon.sock"
SEED_SOCK="$WORK/seed-state/daemon.sock"
for _ in $(seq 1 "$ROUNDS"); do
  "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
    search rss-file --search-timeout 1 > /dev/null 2>&1 || true
  "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
    browse rssseed > /dev/null 2>&1 || true
  "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
    download rssseed 'seed-share\rss-file.bin' --size 30 > /dev/null 2>&1 || true
  "$BIN" --no-config --quiet --daemon "$SEED_SOCK" \
    download rssload 'load-share\rss-file.bin' --size 30 > /dev/null 2>&1 || true
done

sleep 1

python3 - "$WORK" "$OUT" "$ROUNDS" "$SERVER_PORT" <<'PY'
import json
import pathlib
import subprocess
import sys

work, out, rounds, server_port = sys.argv[1:5]
samples = [
    line.split()
    for line in pathlib.Path(work, "samples.txt").read_text().splitlines()
    if len(line.split()) == 2
]
peak_rss_kib = max((int(row[0]) for row in samples), default=0)
peak_threads = max((int(row[1]) for row in samples), default=0)
revision = subprocess.run(
    ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True
).stdout.strip()
observations = {
    "revision": revision,
    "load": {
        "rounds": int(rounds),
        "operations_per_round": ["search", "browse", "download", "upload"],
        "seeds": 1,
        "share_bytes": 30,
        "server_port": int(server_port),
    },
    "observed": {
        "peak_rss_kib": peak_rss_kib,
        "peak_threads": peak_threads,
        "sample_count": len(samples),
    },
    "boundaries": (
        "One measured daemon plus one seeder and soulfind on one host. RSS and "
        "thread count are sampled from the daemon process only, every 100 ms. "
        "This measures lifecycle and mixed-load growth, not an idle-daemon "
        "baseline or a saturation ceiling. Actor mailboxes and the client "
        "operation channel remain unbounded, so no global memory budget is "
        "claimed."
    ),
}
pathlib.Path(out).write_text(json.dumps(observations, indent=2) + "\n")
print(f"daemon-rss: peak {peak_rss_kib} KiB, {peak_threads} threads -> {out}")
PY
