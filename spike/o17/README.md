# O17: the robot's Zenoh router

Checks that a router running the product config (`robot/deck_gateway/config/router.json5`) gives the Deck link what the design needs. Results and the reasons behind `deck_router`: [docs/architecture.md](../../docs/architecture.md) O17 and [docs/devlog.md](../../docs/devlog.md) § 3.4.

| File | Does |
|------|------|
| `run_router.sh [certs]` | Starts `deck_router` (or `ROUTER=rmw` for `rmw_zenohd`) with the product config, passing the TLS paths via `ZENOH_CONFIG_OVERRIDE` the way the launch file will. `EXTRA_OVERRIDE='access_control/enabled=false;low_pass_filter=[]'` gives the control run. |
| `acl_test.py --certs <dir>` | 28 checks with Python Zenoh 1.10.1 (`spike/.venv`): mTLS refusals, access control by certificate CN, snooping and spoofing, size limits at the boundary, queries, liveliness. Needs `ca.pem`, `deck.*`, plus `intruder.*` (robot CA, unlisted CN) and `rogue.*` (other CA, same CN as the Deck). |

```bash
# certs for the deny cases (throwaway, from the spike CA)
d=$(mktemp -d); cp spike/certs/{ca.pem,deck.pem,deck.key} $d
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout $d/intruder.key -subj /CN=intruder-000000 -out $d/i.csr
openssl x509 -req -in $d/i.csr -CA spike/certs/ca.pem -CAkey spike/certs/ca.key -CAcreateserial -CAserial $d/rj.srl -days 7 -out $d/intruder.pem
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout $d/rca.key -subj /CN=rogue-ca -days 7 -out $d/rca.pem
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout $d/rogue.key -subj /CN=deck1-spike -out $d/r.csr
openssl x509 -req -in $d/r.csr -CA $d/rca.pem -CAkey $d/rca.key -CAcreateserial -CAserial $d/r.srl -days 7 -out $d/rogue.pem

spike/o17/run_router.sh &                     # colcon build --packages-select deck_router first
spike/.venv/bin/python spike/o17/acl_test.py --certs $d

# Deck over Wi-Fi through the router: the spike binaries with the product key names
spike/target/release/robot_spike --local tcp/127.0.0.1:7451 --v1-keys
#   on the Deck: echo --v1-keys > ~/rj-spike/args, then launch RJ Spike (or run deck_spike --headless --v1-keys)
sudo spike/scripts/stall.sh 1000              # also 1500, 3000
```

On the Deck, run long headless tests under `systemd-run --user`. A process started over SSH (even with `nohup` or `setsid`) is killed when the session ends.
