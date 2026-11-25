#!/bin/sh
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
set -eu

cd "$(dirname "$0")/.."

SOULFIND_BIN="${SOULFIND_BIN:-$PWD/.work/tmp/soulfind/bin/soulfind}"
BIN="${SOULSEEK_BIN:-$PWD/target/debug/soulseek-rs}"
WORK="${DAEMON_RSS_WORK:-$PWD/.work/tmp/daemon-rss}"
OUT="${DAEMON_RSS_OUT:-$WORK/daemon-rss.json}"
ROUNDS="${DAEMON_RSS_ROUNDS:-8}"
HELPER="$PWD/scripts/daemon_rss_helper.py"

if [ ! -x "$SOULFIND_BIN" ]; then
    printf 'daemon-rss: no soulfind at %s — set SOULFIND_BIN\n' "$SOULFIND_BIN" >&2
    exit 2
fi
if [ ! -x "$BIN" ]; then
    printf 'daemon-rss: no client at %s — set SOULSEEK_BIN\n' "$BIN" >&2
    exit 2
fi

pids=""
cleanup() {
    [ -n "$pids" ] || return 0
    trap - 0 INT TERM
    for pid in $pids; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    pids=""
}
on_signal() {
    cleanup
    trap - INT TERM
    kill -INT "$$"
}
trap cleanup 0
trap 'on_signal INT' INT
trap 'on_signal TERM' TERM

ports=$("$HELPER" ports)
if [ -z "$ports" ]; then
    printf 'daemon-rss: no ports available\n' >&2
    exit 2
fi
read -r SERVER_PORT SEED_LISTEN LOAD_LISTEN <<EOF
$ports
EOF
if [ -z "${LOAD_LISTEN:-}" ]; then
    printf 'daemon-rss: expected three ports, got: %s\n' "$ports" >&2
    exit 2
fi

rm -rf "$WORK"
mkdir -p "$WORK/seed-share" "$WORK/load-share" "$WORK/seed-state" "$WORK/load-state" \
    "$WORK/seed-downloads" "$WORK/load-downloads"
printf 'daemon-rss-fixture-0123456789' > "$WORK/seed-share/rss-file.bin"
printf 'daemon-rss-fixture-0123456789' > "$WORK/load-share/rss-file.bin"

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
    printf 'daemon-rss: measured daemon exited\n' >&2
    exit 3
fi

"$HELPER" sample "$LOAD_PID" "$WORK/samples.txt" &
pids="$pids $!"

LOAD_SOCK="$WORK/load-state/daemon.sock"
SEED_SOCK="$WORK/seed-state/daemon.sock"
i=0
while [ "$i" -lt "$ROUNDS" ]; do
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        search rss-file --search-timeout 1 > /dev/null 2>&1 || true
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        browse rssseed > /dev/null 2>&1 || true
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        download rssseed 'seed-share\rss-file.bin' --size 30 > /dev/null 2>&1 || true
    "$BIN" --no-config --quiet --daemon "$SEED_SOCK" \
        download rssload 'load-share\rss-file.bin' --size 30 > /dev/null 2>&1 || true
    i=$((i + 1))
done

sleep 1
"$HELPER" report "$WORK" "$OUT" "$ROUNDS" "$SERVER_PORT"
