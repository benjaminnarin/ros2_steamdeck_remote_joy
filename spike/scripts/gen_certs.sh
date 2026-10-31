#!/usr/bin/env bash
# Spike-only PKI: a throwaway CA, a robot server cert and one Deck client cert.
# Usage: scripts/gen_certs.sh [ROBOT_IP]   (default 192.168.1.131)
# Writes certs/{ca,robot,deck}.{pem,key}. Not the product `pair` tool.
set -euo pipefail

ROBOT_IP=${1:-192.168.1.131}
OUT="$(cd "$(dirname "$0")/.." && pwd)/certs"
mkdir -p "$OUT"
cd "$OUT"

ec() { echo "-newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes"; }

# shellcheck disable=SC2046
openssl req -x509 $(ec) -keyout ca.key -out ca.pem -days 365 -subj "/CN=rj-spike-ca" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null

issue() { # name cn extensions
  # shellcheck disable=SC2046
  openssl req $(ec) -keyout "$1.key" -out "$1.csr" -subj "/CN=$2" 2>/dev/null
  openssl x509 -req -in "$1.csr" -CA ca.pem -CAkey ca.key -CAcreateserial -out "$1.pem" -days 365 \
    -extfile <(printf '%b' "$3") 2>/dev/null
  rm -f "$1.csr"
}

issue robot rj-spike-robot "subjectAltName=IP:${ROBOT_IP},IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE"
issue deck deck1-spike "extendedKeyUsage=clientAuth\nbasicConstraints=CA:FALSE"
chmod 600 ./*.key

echo "certs in $OUT (robot SAN: $ROBOT_IP, 127.0.0.1)"
