#!/usr/bin/env bash
# Regenerate <name>.bin from <name>.txtpb with protoc (the reference encoder).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
proto_root=$here/../../proto
for txt in "$here"/*.txtpb; do
  type=$(sed -n 's/^# message: *//p' "$txt" | head -1)
  [[ -n $type ]] || { echo "$txt: missing '# message: <type>' line" >&2; exit 1; }
  protoc -I "$proto_root" --encode="$type" "$proto_root"/remote_joy/v1/*.proto <"$txt" >"${txt%.txtpb}.bin"
  echo "$(basename "${txt%.txtpb}.bin"): $type, $(stat -c %s "${txt%.txtpb}.bin") bytes"
done
