# Upstream reports (drafts, not filed)

Issues, PRs and comments we might send to other projects. **Nothing here has been posted.** Each entry is a draft to review at the end of the project, when we'll decide whether to file it. Until then, nobody files or comments upstream on the project's behalf.

For each entry: where it would go, why, the evidence we have, what still needs checking before filing, and a draft title and body. Before filing anything, recheck it against the latest upstream release. Some of these may be fixed by then.

| # | Project | Kind | Summary | Ready to file? |
|---|---------|------|---------|----------------|
| U1 | [ros2/rmw_zenoh](https://github.com/ros2/rmw_zenoh) | Issue | `zenoh_cpp_vendor` locks `quinn-proto` 0.11.17, which crashes routers on QUIC datagrams | Yes, after a recheck |
| U2 | [eclipse-zenoh/zenoh](https://github.com/eclipse-zenoh/zenoh) | Feature request | Warn about unknown endpoint options (e.g. `mixed_rel` on a version without it) | No: confirm the behaviour first |
| U3 | [eclipse-zenoh/zenoh](https://github.com/eclipse-zenoh/zenoh) | Docs | Say that best-effort over QUIC uses streams unless `mixed_rel=1` | Yes, after a recheck |
| U4 | [eclipse-zenoh/zenoh](https://github.com/eclipse-zenoh/zenoh) | Question or issue | ACL "multiple network interfaces" warning appears even with no interface-based subjects | No: needs a minimal repro |

---

## U1: `zenoh_cpp_vendor` locks `quinn-proto` 0.11.17 (quinn #2805 panic)

- **Where:** ros2/rmw_zenoh issues. Possibly also eclipse-zenoh/zenoh-c, since the lock comes from the zenoh-c commit the vendor package pins.
- **Why it matters to us:** it's why we run our own router (`deck_router`) instead of `rmw_zenohd` ([architecture.md](architecture.md) O17). A fix lets us go back.
- **Evidence:**
  - [devlog § 3.4](devlog.md#34-o17-rmw_zenohd-as-the-router-and-why-its-deck_router-instead). `rmw_zenohd` from `ros-lyrical-rmw-zenoh-cpp` 0.10.7 aborted on Wi-Fi with `mixed_rel=1` (panic text below). The same traffic through crates.io Zenoh with `quinn-proto` 0.11.19 was stable.
  - The zenoh-c `Cargo.lock` at the commits pinned by Jazzy (`b8df2bb`) and Lyrical/rolling (`07b0d43`) has `quinn-proto` 0.11.17. zenoh-c `main` has 0.11.19.
- **Check before filing:**
  - the latest `zenoh_cpp_vendor` release on each distro;
  - whether an issue already exists (search "quinn", "desynchronized", "datagram");
  - whether the maintainers would rather have a zenoh-c bump to 1.10.2, which also has QUIC fixes (see below).
- **Draft:**

  > **Title:** zenoh_cpp_vendor: zenoh-c locks quinn-proto 0.11.17, router aborts on QUIC datagrams (quinn #2805)
  >
  > The zenoh-c revision vendored by `zenoh_cpp_vendor` (Jazzy b8df2bb, Lyrical/Rolling 07b0d43) locks `quinn-proto` 0.11.17. That version double-subtracts `payload_bytes` when it evicts outgoing datagrams with `drop = true` (quinn-rs/quinn#2805, fixed in 0.11.18 by #2806). Once the datagram send buffer fills, the process panics:
  >
  > ```
  > thread 'tx-0' panicked at quinn-proto-0.11.17/src/connection/datagrams.rs:47:22:
  > datagrams.outgoing.payload_bytes desynchronized
  > ```
  >
  > Because zenoh-c aborts on panic, `rmw_zenohd` exits.
  >
  > **Reproduce:**
  > - `rmw_zenohd` (`ros-lyrical-rmw-zenoh-cpp` 0.10.7) with a QUIC listener `quic/0.0.0.0:7450?mixed_rel=1`.
  > - A client over Wi-Fi sending best-effort data at 50 Hz, with about 4 Mbps of best-effort data coming back.
  > - Over about 20 s, the round-trip time climbs to seconds as the queue accounting drifts, then the router aborts.
  >
  > On loopback the buffer never fills, so it doesn't reproduce there. The same setup built from crates.io Zenoh with `quinn-proto` 0.11.19 runs stable.
  >
  > **Request:** bump the vendored zenoh-c so its lock file has `quinn-proto` ≥ 0.11.18. zenoh-c `main` already has 0.11.19. Zenoh 1.10.2 also includes QUIC fixes that may be worth picking up: eclipse-zenoh/zenoh#2870 (datagram MTU in mixed reliability) and the QUIC acceptor DoS fix.

---

## U2: Zenoh silently accepts unknown endpoint options

- **Where:** eclipse-zenoh/zenoh, as a feature request.
- **Why it matters to us:** on a Zenoh build without `mixed_rel` (1.8.0, the Lyrical 0.10.6 binary), `?mixed_rel=1` would be ignored. Best-effort data would then go back on QUIC streams, and the stale burst M0 found would return with no error ([devlog § 3.3](devlog.md#33-checking-open-questions-before-the-gateway-a-wrong-zenoh-version)). `deck_router` now checks the endpoint itself, but every Zenoh user faces this.
- **Evidence:**
  - Zenoh 1.8.0's source and the 0.10.6 binary have no `mixed_rel`.
  - That an unknown option is accepted without a warning is **inferred, not observed**: we never ran 1.8.0 with `?mixed_rel=1`.
- **Check before filing:** run a 1.8.0 router (or any version) with a made-up endpoint option such as `?not_an_option=1`, and record what it logs. Check whether newer versions already warn.
- **Draft (adjust to what the check shows):**

  > **Title:** Warn (or fail) on unknown endpoint metadata/config keys
  >
  > Endpoint options such as `?mixed_rel=1` change behaviour significantly. When a build doesn't know an option (e.g. 1.8.0 and `mixed_rel`), it is accepted silently, so a configuration that relies on it quietly falls back to different behaviour. For best-effort traffic over QUIC that means stream delivery and replayed stale data after a stall. Could Zenoh log a warning, or reject the endpoint, when it has an option the link doesn't recognise?

---

## U3: Docs: best-effort over QUIC needs `mixed_rel=1` to use datagrams

- **Where:** Zenoh docs or eclipse-zenoh/zenoh (`DEFAULT_CONFIG.json5` comments, or the QUIC link docs).
- **Why it matters to us:**
  - M0 spent a day finding this. With the default QUIC endpoint, best-effort publishers still went over a reliable stream, and a 1 s Wi-Fi stall replayed 73 queued messages, up to 1.64 s old.
  - Adding a second `?rel=0` endpoint didn't help.
  - Only reading `zenoh-link-quic` showed that `mixed_rel=1` makes best-effort keys use datagrams ([devlog § 2.2](devlog.md#22-tests-and-results)).
- **Evidence:** M0 measurements in [spike/README.md](../spike/README.md#results-2026-10-31).
- **Check before filing:** whether current docs already explain `mixed_rel`, and the exact wording of the option in 1.10.2.
- **Draft:**

  > **Title:** Document that QUIC best-effort only uses datagrams with `mixed_rel=1`
  >
  > With a default QUIC endpoint, publishers declared `Reliability::BestEffort` are still carried on a reliable QUIC stream. After a short stall, the queued data is delivered late, which is what best effort is meant to avoid (e.g. for control or video). In our test, a 1 s stall replayed 73 messages up to 1.64 s old. With `?mixed_rel=1` on both listener and connector, best-effort data uses QUIC datagrams and the same stall gives 0 stale messages. This isn't mentioned in the endpoint docs or `DEFAULT_CONFIG.json5` as far as we could find. A sentence near the QUIC endpoint options, and one near publisher reliability, would have saved us a day.

---

## U4: ACL "multiple network interfaces" warning with no interface-based subjects

- **Where:** eclipse-zenoh/zenoh. Ask first in discussions or Discord if unsure.
- **Why it matters to us:** the warning says the ACL "might incorrectly apply filters". We removed our only `interfaces:` subject because of it, but the warning stayed: three times per test run with `deck_router` (Zenoh 1.10.2). We can't tell whether it's noise or a real risk to our access control. Our tests show the rules working correctly.
- **Evidence:** the `deck_router` log during `spike/o17/acl_test.py`, with `router.json5` matching subjects only on `link_protocols` and `cert_common_names`:

  ```
  WARN zenoh::net::routing::interceptor::access_control: Transport returned multiple network interfaces, current ACL logic might incorrectly apply filters in this case!
  ```
- **Check before filing:** make a minimal config that reproduces it (QUIC on `0.0.0.0`, ACL with only `cert_common_names`, on a host with several interfaces), and read the code that emits it to see what "incorrectly" would mean.
- **Draft:**

  > **Title:** ACL warns "Transport returned multiple network interfaces" when no subject uses `interfaces`
  >
  > With access control enabled and subjects that only use `cert_common_names` / `link_protocols`, a router listening on `quic/0.0.0.0:7450` logs the warning above for each QUIC client on a multi-interface host. If interface matching isn't in use, can the warning be skipped? If it does affect these subjects, what's the risk?
