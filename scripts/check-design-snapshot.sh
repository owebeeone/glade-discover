#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
snapshot="$repo_root/dev-docs/GladeDiscoveryDesign.md"
parent_source="$repo_root/../dev-docs/glade/GladeDiscoveryDesign.md"
decisions="$repo_root/dev-docs/Decisions.md"

if [ ! -f "$snapshot" ]; then
    echo "missing vendored design snapshot: $snapshot" >&2
    exit 1
fi

expected=$(sed -n 's/^Body SHA-256: `\([0-9a-f][0-9a-f]*\)`$/\1/p' "$snapshot")
if [ "${#expected}" -ne 64 ]; then
    echo "snapshot has no valid Body SHA-256 metadata" >&2
    exit 1
fi

if [ ! -f "$decisions" ]; then
    echo "missing independent design-hash anchor: $decisions" >&2
    exit 1
fi
recorded=$(sed -n 's/.*`\([0-9a-f][0-9a-f]*\)`.*/\1/p' "$decisions" | awk 'length($0) == 64 { print; exit }')
if [ "$recorded" != "$expected" ]; then
    echo "snapshot hash is not the accepted hash recorded in Decisions.md" >&2
    echo "snapshot declares $expected, decisions record ${recorded:-<missing>}" >&2
    exit 1
fi

body=$(mktemp "${TMPDIR:-/tmp}/glade-discovery-design.XXXXXX")
trap 'rm -f "$body"' EXIT HUP INT TERM
sed '1,/^<!-- BEGIN VENDORED BODY -->$/d' "$snapshot" > "$body"

actual=$(shasum -a 256 "$body" | awk '{print $1}')
if [ "$actual" != "$expected" ]; then
    echo "vendored design body hash mismatch: expected $expected, got $actual" >&2
    exit 1
fi

if [ -f "$parent_source" ]; then
    parent_hash=$(shasum -a 256 "$parent_source" | awk '{print $1}')
    if [ "$parent_hash" != "$expected" ]; then
        echo "parent design changed; explicitly re-vendor it before continuing" >&2
        echo "expected $expected, parent has $parent_hash" >&2
        exit 1
    fi
fi

echo "design snapshot verified: $expected"
