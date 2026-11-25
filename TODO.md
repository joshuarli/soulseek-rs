Prefix: SOUL
Next: 9

## Working checklist — conservative networking campaign

- [x] Mapping policy (`off`/`auto`/`upnp`/`natpmp`) wired through CLI, env,
      file, help, completions and all session entry points; `off` starts no
      worker; invalid combinations rejected before sockets open.
- [x] Default sharing and peer listener preserved; share roots confined at
      file-open time; `.part` files excluded.
- [x] Resource bounds added/refreshed: frame buffer cap, handshake refusal,
      queue caps, pending-serve TTL, search budgets, reconnect stop.
- [x] Daemon control kept local; non-loopback plaintext TCP refused; token
      created 0600 atomically; socket mode tested.
- [x] Docs, network contract, CI server-backed job, `make bench` target.
- [x] Server-backed verification: local soulfind built from source; full
      `SOULSEEK_E2E_REQUIRED=1` suite passed on macOS with two test threads
      (one earlier run failed only under default parallelism, when 100
      concurrent soulfind instances exhausted this host).
- [ ] Independent-client (Nicotine+) interoperability: GUI client not
      available in this environment; unverified.
- [ ] Public-network smoke test: no purpose-provided credentials; not run.
