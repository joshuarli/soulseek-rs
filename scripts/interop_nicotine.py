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


def main():
    base = os.environ.get("INTEROP_DIR", ".work/tmp/interop")
    cfg = os.environ.get(
        "NICOTINE_CONFIG", os.path.join(base, "nicotine-driver-config")
    )
    data = os.environ.get(
        "NICOTINE_DATA_HOME", os.path.join(base, "nico-driver-data")
    )
    rust_user = os.environ.get("RUST_USER", "rustshare")
    virtual_path = os.environ.get(
        "RUST_VIRTUAL_PATH", "share-rust\\rust-track.mp3"
    )

    from pynicotine.config import config

    config.set_config_file(cfg)
    os.environ["NICOTINE_DATA_HOME"] = data

    from pynicotine.core import core
    from pynicotine.events import events
    from pynicotine.slskmessages import UserStatus

    core.init_components(enabled_components=None)
    core.start()
    core.connect()

    deadline = time.time() + float(os.environ.get("INTEROP_TIMEOUT", "30"))
    while time.time() < deadline:
        events.process_thread_events()
        if core.users.login_status == UserStatus.ONLINE:
            break
        time.sleep(0.1)
    else:
        print("interop: login timeout", file=sys.stderr)
        return 1

    print("interop: logged in as", core.users.login_username, flush=True)

    core.userbrowse.browse_user(rust_user, new_request=True)
    browsed = None
    while time.time() < deadline:
        events.process_thread_events()
        browsed = core.userbrowse.users.get(rust_user)
        if browsed is not None and (browsed.public_folders or browsed.private_folders):
            break
        time.sleep(0.1)
    else:
        print("interop: browse timeout", file=sys.stderr)
        return 2

    for folders in (browsed.public_folders, browsed.private_folders):
        for folder, files in folders.items():
            for entry in files:
                print("interop: FILE", folder, entry, flush=True)
    print("interop: BROWSE OK", flush=True)

    core.downloads.enqueue_download(rust_user, virtual_path)
    transfer = None
    while time.time() < deadline:
        events.process_thread_events()
        transfer = core.downloads.transfers.get(rust_user + virtual_path)
        if transfer is not None and transfer.status == "Finished":
            break
        time.sleep(0.1)

    if transfer is None or transfer.status != "Finished":
        print("interop: download did not finish", file=sys.stderr)
        return 3
    print("interop: DOWNLOAD OK", flush=True)

    core.quit()
    events.emit("quit")
    return 0


if __name__ == "__main__":
    multiprocessing.freeze_support()
    # pynicotine keeps non-daemon event threads alive after quit; a scratch
    # verifier must not hang the caller waiting for them.
    os._exit(main())
