#!/usr/bin/env python3
"""Drive headless Nicotine+ against a running soulseek-rs daemon.

This is the independent-client half of the NX-1 interoperability check. It
logs in as NICOTINE_USER, browses RUST_USER's shares through a real Soulseek
peer connection, downloads RUST_VIRTUAL_PATH, and exits 0 only when the
transfer reaches `Finished`.

Prerequisites:
  * A soulfind server at SOULFIND_ADDR (default 127.0.0.1:55002).
  * A soulseek-rs daemon logged in as RUST_USER (default rustshare), listening
    on a reachable address and sharing RUST_SHARE_DIR.
  * A Python environment with pynicotine installed (NICOTINE_PYTHON).

Nicotine+ config values are Python literals, not TOML: write `login = user`,
not `login = 'user'`. Quotes become part of the login name and peer lookups
then fail with a `0.0.0.0:0` address from the server.
"""

import multiprocessing
import os
import sys
import time
from pathlib import Path


def wait_until(
    condition,
    events,
    timeout: float,
    description: str,
) -> bool:
    """Pump pynicotine events until `condition()` holds or `timeout` elapses."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        events.process_thread_events()
        if condition():
            return True
        time.sleep(0.1)
    print(f"interop: {description} timeout", file=sys.stderr)
    return False


def run() -> int:
    base = Path(os.environ.get("INTEROP_DIR", ".work/tmp/interop"))
    cfg = Path(os.environ.get("NICOTINE_CONFIG", str(base / "nicotine-driver-config")))
    data = Path(os.environ.get("NICOTINE_DATA_HOME", str(base / "nico-driver-data")))
    rust_user = os.environ.get("RUST_USER", "rustshare")
    virtual_path = os.environ.get("RUST_VIRTUAL_PATH", "share-rust\\rust-track.mp3")
    timeout = float(os.environ.get("INTEROP_TIMEOUT", "30"))

    from pynicotine.config import config

    config.set_config_file(str(cfg))
    os.environ["NICOTINE_DATA_HOME"] = str(data)

    from pynicotine.core import core
    from pynicotine.events import events
    from pynicotine.slskmessages import UserStatus

    core.init_components(enabled_components=None)
    core.start()
    core.connect()

    try:
        if not wait_until(
            lambda: core.users.login_status == UserStatus.ONLINE,
            events,
            timeout,
            "login",
        ):
            return 1
        print("interop: logged in as", core.users.login_username, flush=True)

        core.userbrowse.browse_user(rust_user, new_request=True)
        browsed = None

        def loaded() -> bool:
            nonlocal browsed
            browsed = core.userbrowse.users.get(rust_user)
            return browsed is not None and bool(
                browsed.public_folders or browsed.private_folders
            )

        if not wait_until(loaded, events, timeout, "browse"):
            return 2
        if browsed is None:
            return 2
        for folders in (browsed.public_folders, browsed.private_folders):
            for folder, files in folders.items():
                for entry in files:
                    print("interop: FILE", folder, entry, flush=True)
        print("interop: BROWSE OK", flush=True)

        core.downloads.enqueue_download(rust_user, virtual_path)
        transfer = None

        def finished() -> bool:
            nonlocal transfer
            transfer = core.downloads.transfers.get(rust_user + virtual_path)
            return transfer is not None and transfer.status == "Finished"

        if not wait_until(finished, events, timeout, "download"):
            return 3
        print("interop: DOWNLOAD OK", flush=True)
        return 0
    finally:
        core.quit()
        events.emit("quit")


if __name__ == "__main__":
    multiprocessing.freeze_support()
    code = run()
    # pynicotine keeps non-daemon event threads alive after quit; a scratch
    # verifier must not hang the caller waiting for them.
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(code)
