#!/bin/bash
# Build and run the Swift client against the live daemon.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$(mktemp -d)/protocol-test"
swiftc -O -parse-as-library \
  -o "$OUT" \
  "$HERE/Tests/ProtocolTest.swift" \
  "$HERE/Sources/Models.swift" \
  "$HERE/Sources/Client.swift"
"$OUT"
