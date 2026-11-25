# Benchmarks

## Frame draw and message parse

Two deterministic benchmarks cover the paths the mixed-load workload cannot
attribute: a full-window draw of the 120x40 fixture session through
`TestBackend`, and a parse of a captured `FileSearchResponse` frame. Each runs a
fixed number of iterations, reports the median of 21 batches in ns/iter, and
needs no server. Neither is part of `make test` or CI.

```sh
cargo bench -p soulseek-rs-lib --bench message_parse
cargo bench -p soulseek-rs --features fixtures --bench tui_frame
```

## Concurrency benchmark

Answers one question: how much simultaneous Soulseek activity can the client
absorb before it starts losing work?

```sh
scripts/stress.sh           # run the default profile
scripts/stress.sh --save    # ...and store it as the new baseline if it wins
```

A run takes about 7 seconds. It spawns a local soulfind, surrounds one real
`Client` with a swarm of mock peers, and drives searches, downloads, uploads and
browses **at the same time** — the mixing is the point, since every ceiling found
so far only appeared under a mixed load.

## The score

One number out of 100, weighted across five dimensions:

| Dimension  | Weight | Measures                                        |
|------------|--------|-------------------------------------------------|
| downloads  | 0.30   | completed / started                             |
| uploads    | 0.20   | served / requested                              |
| searches   | 0.20   | queries that got at least one result            |
| browses    | 0.15   | browse requests answered                        |
| throughput | 0.15   | aggregate MiB/s against `TARGET_MBPS` (500)     |

The four functional dimensions should stay at 100%: anything less means work was
silently dropped, which is the failure mode that matters. Throughput is the term
with headroom, so the score still moves when transfer handling regresses.

`--save` only overwrites `stress-baseline.json` when the run beats it, so the
baseline ratchets upward. Without `--save` a run just prints the delta.

## The gate

`STRESS_REQUIRE_FUNCTIONAL=1` makes the run exit 1 when any of the four
functional dimensions is below 100%; `make bench` is the local entrypoint and
requires `SOULFIND_BIN`. CI does **not** run the stress workload: it needs a
local server and measures the machine as much as the client, so throughput is
never a gate.

### Local before/after check

A release-mode, single-process share workload (4,000 synthetic files across 40
folders, one scan, 2,000 search rounds) was run three times on the same macOS
host against `main` and against the conservative-networking branch, interleaved:

| Build | Median wall | Median user CPU | Peak RSS |
| --- | --- | --- | --- |
| `main` | 0.41 s | 0.06 s | ~3.87 MiB |
| campaign | 0.40 s | 0.07 s | ~5.26 MiB |

The extra ~1.4 MiB is the per-file root/relative identity kept to confine
upload opens; matching time is unchanged within run-to-run spread. This
workload has no peers or server, so it says nothing about transfer throughput,
sockets or thread counts under the mixed-load `stress` profile. Those need a
local soulfind and are not available as a repeatable before/after baseline here.

## Tuning the load

Every knob is an env var; the defaults are in `Config::from_env`.

```sh
STRESS_PEERS=2048 STRESS_FILE_KB=8192 scripts/stress.sh
```

`STRESS_PEERS` `STRESS_SEARCHES` `STRESS_DOWNLOADS` `STRESS_LEECHERS`
`STRESS_BROWSES` `STRESS_FILE_KB` `STRESS_TIMEOUT`

## Reading a failure

`downloads completed 480/512` is the headline, but the two lines under it say
where to look:

- **`download states`** — a straggler on `Queued` never got its transfer
  negotiated; one on `InProgress` is merely slow. These are different bugs.
- **`mocks never online`** — mock peers the harness could not bring up. That is
  a harness/server limit, not the client, and it is reported separately so it
  cannot quietly shrink a denominator and flatter the score.

`peak threads` counts the whole process — client *and* mock swarm — so treat it
as a trend, not a client measurement.

`LOG_LEVEL=debug scripts/stress.sh` correlates a stuck peer's name against the
client's own trace, which is how each ceiling below was actually found.

## Ceilings found and removed

| Symptom                                            | Cause                                                                         |
|----------------------------------------------------|-------------------------------------------------------------------------------|
| Hard stop at ~15 peers; later peers never start    | Every `PeerActor` held a `ThreadPool` worker for life, so the pool capped peers at one per core |
| Downloads stuck on `Queued` under load             | `TransferResponse` was sent before the client recorded the peer's token, so the file connection raced its own bookkeeping |
| One download vanishing per few hundred             | Token was `md5(filename)`, and the store deletes *every* entry with a token — same filename from two peers destroyed both |
| Uploads dropped after the peer accepted            | Server answered `GetPeerAddress` with port 0; the client cached it and dialled it |

## Resource envelope — measured and theoretical

The deterministic benchmarks and `scripts/stress.sh` cover one frame draw, one
parse, and a mixed network load. They do not establish a laptop-wide memory
bound. Actor mailboxes and the client operation channel are still unbounded,
and 512 allowed peers times a 16 MiB receive-buffer allowance is an 8 GiB
*allowance*, not measured RSS. Queue admission limits apply after work has
entered those channels. These remain open work, not a proven resource budget.

Daemon control-listener tests start and stop listeners through an owning guard;
repeated runs stayed between 26 and 29 MiB maximum RSS with no thread growth.
That is a lifecycle regression check, not an idle-daemon or saturation
measurement.

### Reproducible daemon RSS workload

```sh
SOULFIND_BIN=/path/to/soulfind scripts/daemon_rss.sh
DAEMON_RSS_ROUNDS=16 scripts/daemon_rss.sh
```

The script starts a local soulfind, a seeder daemon, and the measured daemon,
then drives one search, browse, download and upload per round in both
directions, sampling the measured daemon's RSS and thread count every 100 ms.
It writes machine-readable JSON with the exact git revision, load parameters,
peak RSS, peak threads, sample count and measurement boundaries. Default output
is `.work/tmp/daemon-rss/daemon-rss.json`; set `DAEMON_RSS_OUT` to keep a copy.

At revision `45d53c2` on this macOS host, 8 rounds against debug binaries gave
a peak of 9456 KiB RSS and 12 threads over 121 samples. Fixture files were 30
bytes and both downloads completed; the result is a growth check under a small
real load, not an idle baseline or a saturation ceiling.

The mixed-load harness also reports peak RSS and the git revision that produced
a stored baseline:

```sh
STRESS_PEERS=64 STRESS_SEARCHES=16 STRESS_DOWNLOADS=16 \
STRESS_LEECHERS=8 STRESS_BROWSES=8 STRESS_FILE_KB=64 scripts/stress.sh
```

Its RSS and thread count cover the client and the mock swarm in one process, so
they are an upper bound for the client alone and are not comparable with the
daemon figures above. At revision `45d53c2` a reduced profile (64 peers, 16
searches, 16 downloads, 8 leechers, 8 browses, 64 KiB files) reported peak RSS
8384 KiB and 147 threads: searches, downloads and browses were 100%, uploads
3/8 and only 26/64 mock seeders came online. That is a harness-side load
limitation, not a client ceiling; the stored full-profile baseline has all four
functional dimensions at 100%.

## Requirements

A soulfind binary (`SOULFIND_BIN`, defaulted in `scripts/stress.sh`), or
`SOULSEEK_TEST_SERVER=host:port` to point at a running server. The script raises
the descriptor limit — a swarm holds a lot of sockets open.
