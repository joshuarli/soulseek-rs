#!/usr/bin/env python3
"""Helpers for scripts/daemon_rss.sh: free ports, process sampling, and JSON.

The sampler supports macOS and Linux, the two platforms this repository runs
on. Other platforms report zero counters with a warning rather than a value
in the wrong unit.
"""

import ctypes
import json
import os
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
    """Peak-independent RSS and thread count for `pid`, in KiB and threads."""
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


def report(work: Path, output: Path, rounds: int, server_port: int) -> None:
    """Write the machine-readable measurement envelope to `output`."""
    rows: list[tuple[int, int]] = []
    for line in (work / "samples.txt").read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2:
            rows.append((int(parts[0]), int(parts[1])))
    observed = {
        "revision": revision(),
        "load": {
            "rounds": rounds,
            "operations_per_round": ["search", "browse", "download", "upload"],
            "seeds": 1,
            "share_bytes": 30,
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
        "daemon-rss: peak {rss} KiB, {threads} threads -> {out}".format(
            rss=observed["observed"]["peak_rss_kib"],
            threads=observed["observed"]["peak_threads"],
            out=output,
        )
    )


def main(args: list[str]) -> int:
    """Dispatch the shell script's helper commands."""
    if args == ["ports"]:
        print(*free_ports(3))
        return 0
    if len(args) == 3 and args[0] == "sample":
        sample(int(args[1]), Path(args[2]))
        return 0
    if len(args) == 5 and args[0] == "report":
        report(Path(args[1]), Path(args[2]), int(args[3]), int(args[4]))
        return 0
    print("usage: daemon_rss_helper.py ports|sample PID OUT|report WORK OUT ROUNDS PORT", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
