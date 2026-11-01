#!/usr/bin/env python3
"""O17: check rmw_zenohd + robot/deck_gateway/config/router.json5 (access control, size limits, mTLS).

Run with spike/o17/run_router.sh already running. Plays these sessions:
  local     client on tcp/127.0.0.1:7451 (stands in for deck_gateway / deck_video)
  deck      client on quic/127.0.0.1:7450?mixed_rel=1, paired cert (CN in the allow list)
  deck2     a second session with the paired cert (another app run or another paired Deck)
  intruder  cert signed by the robot CA, but CN not in the allow list
  rogue     cert from a different CA          -> must not connect
  nocert    no client certificate             -> must not connect
Payloads are dummy bytes: the router doesn't parse them.
"""

import argparse
import sys
import threading
import time
from pathlib import Path

import zenoh

WAIT = 0.6  # s to wait for something that should (or shouldn't) arrive
results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    print(f"  {'PASS' if ok else 'FAIL'}  {name}{'  (' + detail + ')' if detail else ''}")


def cfg_local(port):
    c = zenoh.Config()
    for k, v in [("mode", '"client"'), ("connect/endpoints", f'["tcp/127.0.0.1:{port}"]'),
                 ("scouting/multicast/enabled", "false")]:
        c.insert_json5(k, v)
    return c


def cfg_deck(port, ca, cert=None, key=None):
    c = zenoh.Config()
    for k, v in [("mode", '"client"'),
                 ("connect/endpoints", f'["quic/127.0.0.1:{port}?mixed_rel=1"]'),
                 ("connect/timeout_ms", "3000"),
                 ("scouting/multicast/enabled", "false"),
                 ("transport/link/tls/root_ca_certificate", f'"{ca}"')]:
        c.insert_json5(k, v)
    if cert:
        c.insert_json5("transport/link/tls/enable_mtls", "true")
        c.insert_json5("transport/link/tls/connect_certificate", f'"{cert}"')
        c.insert_json5("transport/link/tls/connect_private_key", f'"{key}"')
    return c


class Inbox:
    """Collects samples from a subscriber callback."""

    def __init__(self):
        self.items = []
        self.lock = threading.Lock()

    def __call__(self, sample):
        with self.lock:
            self.items.append((str(sample.key_expr), len(sample.payload.to_bytes()), getattr(sample, "kind", None)))

    def take(self, wait=WAIT):
        time.sleep(wait)
        with self.lock:
            out, self.items = self.items, []
        return out


def try_open(cfg):
    try:
        return zenoh.open(cfg), None
    except Exception as e:  # noqa: BLE001 - any failure to open counts as "not connected"
        return None, str(e).splitlines()[0][:100]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--certs", required=True, help="dir with ca.pem, deck.pem/key, intruder.pem/key, rogue.pem/key")
    p.add_argument("--quic-port", type=int, default=7450)
    p.add_argument("--local-port", type=int, default=7451)
    a = p.parse_args()
    d = Path(a.certs)
    ca = d / "ca.pem"

    print("== connections")
    local, err = try_open(cfg_local(a.local_port))
    check("local session connects on tcp loopback", local is not None, err or "")
    deck, err = try_open(cfg_deck(a.quic_port, ca, d / "deck.pem", d / "deck.key"))
    check("paired Deck connects (QUIC + mTLS)", deck is not None, err or "")
    deck2, err = try_open(cfg_deck(a.quic_port, ca, d / "deck.pem", d / "deck.key"))
    intruder, err = try_open(cfg_deck(a.quic_port, ca, d / "intruder.pem", d / "intruder.key"))
    check("intruder (robot CA, unlisted CN) connects at TLS level", intruder is not None, err or "")
    rogue, err = try_open(cfg_deck(a.quic_port, ca, d / "rogue.pem", d / "rogue.key"))
    check("rogue CA certificate is refused", rogue is None, err or "it connected")
    nocert, err = try_open(cfg_deck(a.quic_port, ca))
    check("no client certificate is refused", nocert is None, err or "it connected")
    if not (local and deck and deck2 and intruder):
        sys.exit("cannot continue without the local, deck, deck2 and intruder sessions")
    time.sleep(0.5)

    print("== Deck -> robot (control)")
    ctl = Inbox()
    local_ctl = local.declare_subscriber("rj/v1/control", ctl)
    time.sleep(0.5)
    deck.put("rj/v1/control", b"x" * 40)
    check("paired Deck control reaches local", len(ctl.take()) == 1)
    intruder.put("rj/v1/control", b"x" * 40)
    check("intruder control is blocked", len(ctl.take()) == 0)
    deck.put("rj/v1/control", b"x" * 1024)
    check("1024 B control passes (limit is inclusive)", len(ctl.take()) == 1)
    deck.put("rj/v1/control", b"x" * 1025)
    check("1025 B control is dropped (size limit)", len(ctl.take()) == 0)
    snoop = Inbox()
    deck2_snoop = deck2.declare_subscriber("rj/v1/control", snoop)
    time.sleep(0.3)
    deck.put("rj/v1/control", b"x" * 40)
    ctl.take()
    check("another Deck can't snoop control", len(snoop.take(0.2)) == 0)
    deck.put("rj/v1/stop", b"s" * 30)
    stop = Inbox()
    local_stop = local.declare_subscriber("rj/v1/stop", stop)
    time.sleep(0.3)
    deck.put("rj/v1/stop", b"s" * 30)
    check("paired Deck stop reaches local", len(stop.take()) == 1)

    print("== robot -> Deck (state, status, video)")
    st_deck, st_intr = Inbox(), Inbox()
    s1 = deck.declare_subscriber("rj/v1/robot_state", st_deck)
    s2 = intruder.declare_subscriber("rj/v1/robot_state", st_intr)
    time.sleep(0.5)
    local.put("rj/v1/robot_state", b"r" * 33)
    check("robot_state reaches the paired Deck", len(st_deck.take()) == 1)
    check("intruder can't subscribe to robot_state", len(st_intr.take(0.1)) == 0)
    spoof_local, spoof_deck2 = Inbox(), Inbox()
    s3 = local.declare_subscriber("rj/v1/robot_state", spoof_local)
    s4 = deck2.declare_subscriber("rj/v1/robot_state", spoof_deck2)
    time.sleep(0.4)
    deck.put("rj/v1/robot_state", b"fake")
    check("a Deck can't spoof robot_state (local)", len(spoof_local.take()) == 0)
    check("a Deck can't spoof robot_state (other Deck)", len(spoof_deck2.take(0.1)) == 0)
    st_deck.take(0)

    vid = Inbox()
    s5 = deck.declare_subscriber("rj/v1/video", vid)
    time.sleep(0.4)
    local.put("rj/v1/video", b"v" * 400_000)
    check("400 KB video frame reaches the Deck", len(vid.take(1.0)) == 1)
    local.put("rj/v1/video", b"v" * 600_000)
    check("600 KB video frame is dropped (512 KB limit)", len(vid.take(1.0)) == 0)
    stat = Inbox()
    s6 = deck.declare_subscriber("rj/v1/status", stat)
    time.sleep(0.4)
    local.put("rj/v1/status", b"s" * 60_000)
    check("60 KB status reaches the Deck", len(stat.take()) == 1)
    local.put("rj/v1/status", b"s" * 70_000)
    check("70 KB status is dropped (64 KB limit)", len(stat.take()) == 0)
    other = Inbox()
    s7 = deck.declare_subscriber("rj/v1/secret", other)
    time.sleep(0.3)
    local.put("rj/v1/secret", b"x")
    check("a Deck can't subscribe to keys outside its list", len(other.take()) == 0)

    print("== queries")
    def on_info(query):
        query.reply("rj/v1/info", b"info-reply")
    q1 = local.declare_queryable("rj/v1/info", on_info)
    def on_action(query):
        query.reply(str(query.key_expr), b"done")
    q2 = local.declare_queryable("rj/v1/action/*", on_action)
    time.sleep(0.5)

    def replies(sess, key, payload=None):
        out = []
        for r in sess.get(key, payload=payload, timeout=1.5):
            if r.ok is not None:
                out.append(r.ok.payload.to_bytes())
        return out

    check("paired Deck gets an info reply", replies(deck, "rj/v1/info") == [b"info-reply"])
    check("paired Deck can run an action query", replies(deck, "rj/v1/action/dock") == [b"done"])
    check("intruder gets no info reply", replies(intruder, "rj/v1/info") == [])
    check("oversized query payload (2 KB) is dropped", replies(deck, "rj/v1/info", b"q" * 2048) == [])

    print("== liveliness")
    seen = Inbox()
    lsub = local.liveliness().declare_subscriber("rj/v1/deck/*", seen)
    time.sleep(0.4)
    tok = deck.liveliness().declare_token("rj/v1/deck/0000abcd")
    got = seen.take()
    check("paired Deck token is seen by local", any(k == "rj/v1/deck/0000abcd" for k, _, _ in got), str(got))
    tok_i = intruder.liveliness().declare_token("rj/v1/deck/0000beef")
    check("intruder token is not seen", seen.take() == [])
    tok.undeclare()
    got = seen.take()
    check("token withdrawal is seen (deliberate disconnect)", len(got) == 1 and "DELETE" in str(got[0][2]).upper(), str(got))
    tok2 = deck.liveliness().declare_token("rj/v1/deck/0000c0de")
    seen.take()
    deck.close()
    got = seen.take(1.0)
    check("closing the Deck session removes its token", len(got) == 1, str(got))

    for s in [deck2, intruder, local]:
        s.close()
    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
