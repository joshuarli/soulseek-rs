#!/usr/bin/env python3
"""Helpers for scripts/daemon_rss.sh: ports, readiness, sampling, verification.

The sampler supports macOS and Linux, the two platforms this repository runs
on. Other platforms report zero counters with a warning rather than a value
in the wrong unit.
"""

import ctypes
import hashlib
import json
import os
import platform
import socket
import subprocess
import sys
import time
from pathlib import Path

SAMPLE_INTERVAL = 0.1


class ProcTaskInfo(ctypes.Structure):
    """The subset of Darwin's `proc_taskinfo` the sampler reads."""

    _fields_ = [
        ("pti_virtual_size", ctypes.c_uint64),
        ("pti_resident_size", ctypes.c_uint64),
        ("pti_total_user", ctypes.c_uint64),
        ("pti_total_system", ctypes.c_uint64),
        ("pti_threads_user", ctypes.c_uint64),
        ("pti_threads_system", ctypes.c_uint64),
        ("pti_policy", ctypes.c_int32),
        ("pti_faults", ctypes.c_int32),
        ("pti_pageins", ctypes.c_int32),
        ("pti_cow_faults", ctypes.c_int32),
        ("pti_messages_sent", ctypes.c_int32),
        ("pti_messages_received", ctypes.c_int32),
        ("pti_syscalls_mach", ctypes.c_int32),
        ("pti_syscalls_unix", ctypes.c_int32),
        ("pti_csw", ctypes.c_int32),
        ("pti_threadnum", ctypes.c_int32),
        ("pti_numrunning", ctypes.c_int32),
        ("pti_priority", ctypes.c_int32),
    ]


def free_ports(count: int) -> list[int]:
    """Ask the OS for `count` distinct ephemeral loopback ports."""
    ports: list[int] = []
    for _ in range(count):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            ports.append(int(sock.getsockname()[1]))
    return ports


def ready(host: str, port: int, timeout: float) -> bool:
    """Whether a TCP listener accepts on `host:port` before `timeout`."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.1)
    return False


def sha256_file(path: Path) -> str:
    """Hash a file's bytes without shelling out to a platform tool."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_transfers(work: Path, rounds: int) -> int:
    """Check both directions copied every distinct fixture byte-for-byte."""
    for round_no in range(rounds):
        name = f"rss-file-{round_no:03}.bin"
        expected = sha256_file(work / "seed-share" / name)
        for side in ("seed-downloads", "load-downloads"):
            received = work / side / name
            if not received.is_file():
                print(f"daemon-rss: missing {received}", file=sys.stderr)
                return 1
            if sha256_file(received) != expected:
                print(f"daemon-rss: corrupt {received}", file=sys.stderr)
                return 1
    return 0


def identity(binary: Path, profile: str, output: Path) -> None:
    """Record which binary, build and environment the evidence describes."""
    version = subprocess.run(
        [str(binary), "--version"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    record = {
        "binary": str(binary),
        "sha256": sha256_file(binary),
        "version": version,
        "build_profile": profile,
        "environment": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
    }
    output.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


def linux_counters(pid: int) -> tuple[int, int]:
    """Resident KiB and thread count from Linux's process status file."""
    fields: dict[str, str] = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        name, _, value = line.partition(":")
        fields[name] = value.strip()
    return int(fields["VmRSS"].split()[0]), int(fields["Threads"])


def darwin_counters(pid: int) -> tuple[int, int]:
    """Resident KiB and thread count from Darwin's task-info syscall."""
    libc = ctypes.CDLL(None)
    libc.proc_pidinfo.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    libc.proc_pidinfo.restype = ctypes.c_int
    info = ProcTaskInfo()
    proc_pid_task_info = 4
    written = libc.proc_pidinfo(
        pid,
        proc_pid_task_info,
        0,
        ctypes.byref(info),
        ctypes.sizeof(info),
    )
    if written <= 0:
        return 0, 0
    return int(info.pti_resident_size // 1024), int(info.pti_threadnum)


def counters(pid: int) -> tuple[int, int]:
    """RSS and thread count for `pid`, in KiB and threads."""
    if sys.platform == "darwin":
        return darwin_counters(pid)
    if sys.platform.startswith("linux"):
        return linux_counters(pid)
    print("daemon-rss: process counters support macOS and Linux only", file=sys.stderr)
    return 0, 0


def alive(pid: int) -> bool:
    """Whether `pid` still exists, without signalling it."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def sample(pid: int, output: Path) -> int:
    """Write `rss_kib threads` lines until `pid` exits; return the count."""
    samples = 0
    with output.open("w", encoding="utf-8") as handle:
        while alive(pid):
            try:
                rss_kib, threads = counters(pid)
            except (OSError, ValueError, KeyError, IndexError) as error:
                print(f"daemon-rss: sample skipped: {error}", file=sys.stderr)
                time.sleep(SAMPLE_INTERVAL)
                continue
            handle.write(f"{rss_kib} {threads}\n")
            handle.flush()
            samples += 1
            time.sleep(SAMPLE_INTERVAL)
    return samples


def revision() -> str:
    """The git commit the measurement was taken at."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def report(
    work: Path,
    output: Path,
    rounds: int,
    server_port: int,
    operations: int,
    binary_record: Path,
) -> None:
    """Write the machine-readable measurement envelope to `output`."""
    fixture_bytes = sum(
        path.stat().st_size for path in (work / "seed-share").iterdir()
    )
    rows: list[tuple[int, int]] = []
    for line in (work / "samples.txt").read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2:
            rows.append((int(parts[0]), int(parts[1])))
    observed = {
        "revision": revision(),
        "executed": json.loads(binary_record.read_text(encoding="utf-8")),
        "load": {
            "rounds": rounds,
            "operations_succeeded": operations,
            "operations_required": rounds * 4,
            "operations_per_round": ["search", "browse", "download", "upload"],
            "distinct_fixture_bytes": fixture_bytes,
            "verified_in_both_directions": True,
            "server_port": server_port,
        },
        "observed": {
            "peak_rss_kib": max((rss for rss, _ in rows), default=0),
            "peak_threads": max((threads for _, threads in rows), default=0),
            "sample_count": len(rows),
        },
        "boundaries": (
            "One measured daemon plus one seeder and soulfind on one host. "
            "RSS and thread count are sampled from the daemon process only, "
            "every 100 ms. This measures lifecycle and mixed-load growth, not "
            "an idle-daemon baseline or a saturation ceiling. Actor mailboxes "
            "and the client operation channel remain unbounded, so no global "
            "memory budget is claimed."
        ),
    }
    output.write_text(json.dumps(observed, indent=2) + "\n", encoding="utf-8")
    print(
        "daemon-rss: peak {rss} KiB, {threads} threads, {ops} operations -> {out}".format(
            rss=observed["observed"]["peak_rss_kib"],
            threads=observed["observed"]["peak_threads"],
            ops=operations,
            out=output,
        )
    )


def main(args: list[str]) -> int:
    """Dispatch the shell script's helper commands."""
    if args == ["ports"]:
        print(*free_ports(3))
        return 0
    if len(args) == 2 and args[0] == "nap":
        time.sleep(float(args[1]))
        return 0
    if len(args) == 4 and args[0] == "ready":
        return 0 if ready(args[1], int(args[2]), float(args[3])) else 1
    if len(args) == 3 and args[0] == "sample":
        sample(int(args[1]), Path(args[2]))
        return 0
    if len(args) == 3 and args[0] == "verify":
        return verify_transfers(Path(args[1]), int(args[2]))
    if len(args) == 4 and args[0] == "identity":
        identity(Path(args[1]), args[2], Path(args[3]))
        return 0
    if len(args) == 7 and args[0] == "report":
        report(
            Path(args[1]),
            Path(args[2]),
            int(args[3]),
            int(args[4]),
            int(args[5]),
            Path(args[6]),
        )
        return 0
    print(
        "usage: daemon_rss_helper.py ports|ready HOST PORT TIMEOUT|"
        "sample PID OUT|verify WORK ROUNDS|identity BIN PROFILE OUT|"
        "report WORK OUT ROUNDS PORT OPS IDENTITY",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
