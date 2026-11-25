#!/bin/sh
# Reproducible daemon RSS envelope under a mixed local Soulseek load.
#
#   scripts/daemon_rss.sh
#   DAEMON_RSS_ROUNDS=8 scripts/daemon_rss.sh
#
# Starts a local soulfind, a seeder daemon, and the measured daemon, checks
# that all three are ready, then drives one search, browse, download and
# upload per round concurrently, with a distinct fixture per round. Every
# operation must succeed and both copies must match the source bytes or the
# run fails without producing acceptance evidence. The result records the
# executed binary's identity, build profile, environment, git revision,
# operation counts and peak RSS/thread count.
set -eu

cd "$(dirname "$0")/.."

SOULFIND_BIN="${SOULFIND_BIN:-$PWD/.work/tmp/soulfind/bin/soulfind}"
BIN="${SOULSEEK_BIN:-$PWD/target/debug/soulseek-rs}"
WORK="${DAEMON_RSS_WORK:-$PWD/.work/tmp/daemon-rss}"
OUT="${DAEMON_RSS_OUT:-$WORK/daemon-rss.json}"
ROUNDS="${DAEMON_RSS_ROUNDS:-8}"
PROFILE="${DAEMON_RSS_PROFILE:-debug}"
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

i=0
while [ "$i" -lt "$ROUNDS" ]; do
    name=$(printf 'rss-file-%03d.bin' "$i")
    printf 'daemon-rss-fixture-%s-%s' "$name" "$i" > "$WORK/seed-share/$name"
    printf 'daemon-rss-fixture-%s-%s' "$name" "$i" > "$WORK/load-share/$name"
    i=$((i + 1))
done

"$SOULFIND_BIN" -p "$SERVER_PORT" -d "$WORK/soulfind.db" > "$WORK/soulfind.log" 2>&1 &
pids="$pids $!"
if ! "$HELPER" ready 127.0.0.1 "$SERVER_PORT" 10; then
    printf 'daemon-rss: server never listened\n' >&2
    exit 3
fi

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

LOAD_SOCK="$WORK/load-state/daemon.sock"
SEED_SOCK="$WORK/seed-state/daemon.sock"
i=0
while [ "$i" -lt 200 ]; do
    if [ -S "$LOAD_SOCK" ] && [ -S "$SEED_SOCK" ] \
        && "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" daemon status > /dev/null 2>&1 \
        && "$BIN" --no-config --quiet --daemon "$SEED_SOCK" daemon status > /dev/null 2>&1; then
        break
    fi
    i=$((i + 1))
    sleep 0.1
done
if [ "$i" -ge 200 ] || ! kill -0 "$LOAD_PID" 2>/dev/null; then
    printf 'daemon-rss: daemons never became ready\n' >&2
    exit 3
fi
# Give the server time to register both sessions' shares before searching.
sleep 2

"$HELPER" identity "$BIN" "$PROFILE" "$WORK/identity.json"
"$HELPER" sample "$LOAD_PID" "$WORK/samples.txt" &
pids="$pids $!"

operations=0
i=0
while [ "$i" -lt "$ROUNDS" ]; do
    name=$(printf 'rss-file-%03d.bin' "$i")
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        search rss-file --search-timeout 10 > "$WORK/search-$i.txt" 2>&1 &
    p1=$!
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        browse rssseed > "$WORK/browse-$i.txt" 2>&1 &
    p2=$!
    "$BIN" --no-config --quiet --daemon "$LOAD_SOCK" \
        download rssseed "seed-share\\$name" -t 20 > "$WORK/download-$i.txt" 2>&1 &
    p3=$!
    "$BIN" --no-config --quiet --daemon "$SEED_SOCK" \
        download rssload "load-share\\$name" -t 20 > "$WORK/upload-$i.txt" 2>&1 &
    p4=$!

    succeeded=0
    for job in "$p1" "$p2" "$p3" "$p4"; do
        if wait "$job"; then
            succeeded=$((succeeded + 1))
        else
            printf 'daemon-rss: workload operation failed (round %s, pid %s)\n' "$i" "$job" >&2
        fi
    done
    operations=$((operations + succeeded))
    if [ "$succeeded" -ne 4 ]; then
        printf 'daemon-rss: required workload failed; no evidence written\n' >&2
        exit 4
    fi
    if [ ! -s "$WORK/search-$i.txt" ] \
        || ! grep -F -q "$name" "$WORK/browse-$i.txt"; then
        printf 'daemon-rss: search or browse returned no result for %s\n' "$name" >&2
        exit 4
    fi
    i=$((i + 1))
done

sleep 1
if ! "$HELPER" verify "$WORK" "$ROUNDS"; then
    printf 'daemon-rss: transfer bytes did not match; no evidence written\n' >&2
    exit 4
fi
"$HELPER" report "$WORK" "$OUT" "$ROUNDS" "$SERVER_PORT" "$operations" "$WORK/identity.json"
