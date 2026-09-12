#!/bin/sh
set -eu

contract_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -gt 1 ]; then
    echo "usage: sh scripts/check-contracts.sh [all|transport|signature|store|acceptance|registry|trust|placement]" >&2
    exit 2
fi
case "${1:-all}" in
    all) set -- -p glade-discover-transport-api -p glade-discover-signature-api -p glade-discover-operation-store-api -p glade-discover-acceptance-api -p glade-discover-registry-api -p glade-discover-trust-api -p glade-discover-placement-api ;;
    transport) set -- -p glade-discover-transport-api ;;
    signature) set -- -p glade-discover-signature-api ;;
    store) set -- -p glade-discover-operation-store-api ;;
    acceptance) set -- -p glade-discover-acceptance-api ;;
    registry) set -- -p glade-discover-registry-api ;;
    trust) set -- -p glade-discover-trust-api ;;
    placement) set -- -p glade-discover-placement-api ;;
    *) echo "usage: sh scripts/check-contracts.sh [all|transport|signature|store|acceptance|registry|trust|placement]" >&2; exit 2 ;;
esac
sh "$contract_root/scripts/check-architecture.sh"
exec cargo test --locked --manifest-path "$contract_root/Cargo.toml" --all-features "$@"
