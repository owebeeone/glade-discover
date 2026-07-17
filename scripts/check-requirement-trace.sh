#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
trace="$repo_root/dev-docs/RequirementTrace.md"

if [ ! -f "$trace" ]; then
    echo "missing requirement trace: $trace" >&2
    exit 1
fi

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/glade-discovery-trace.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

sed -n 's/^| \(s-disc-[a-z0-9-]*\) | primary |.*$/\1/p' "$trace" | sort > "$tmp_dir/actual"
cat > "$tmp_dir/expected" <<'EOF'
s-disc-append-restart
s-disc-authz-boundary
s-disc-corr-collision
s-disc-delayed-sign
s-disc-dos-bound
s-disc-epoch-tie
s-disc-inst-authority
s-disc-inst-forged-node
s-disc-inst-revoked-exec
s-disc-inst-wrong-def
s-disc-no-ping-pong
s-disc-noclaim-handoff
s-disc-owner-proof
s-disc-proof-late
s-disc-regrant
s-disc-restart-uncertain
s-disc-revoke-then-grant
s-disc-route-terminal
s-disc-skew
s-disc-sync-drop
s-disc-sync-retry
s-disc-sync-round
s-disc-unauth-revoke
s-disc-wall-rollback
EOF

if duplicates=$(uniq -d "$tmp_dir/actual") && [ -n "$duplicates" ]; then
    echo "duplicate primary scenario rows:" >&2
    echo "$duplicates" >&2
    exit 1
fi

if ! diff -u "$tmp_dir/expected" "$tmp_dir/actual"; then
    echo "requirement trace primary scenarios do not match the v3.1 set" >&2
    exit 1
fi

for invariant in INV-D0 INV-D1 INV-D2 INV-D3 INV-D4 INV-D5 INV-D6; do
    if ! grep -q "^| $invariant |" "$trace"; then
        echo "missing invariant ownership row: $invariant" >&2
        exit 1
    fi
done

echo "requirement trace verified: 24 primary scenarios, INV-D0..INV-D6 owned"
