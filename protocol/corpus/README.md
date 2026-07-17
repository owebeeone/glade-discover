# Discovery protocol corpus

Files are hexadecimal canonical-CBOR vectors. `valid/` values MUST decode and
round-trip byte-for-byte. `invalid/` values name the fail-closed rejection they
exercise; they MUST never be normalized into accepted input.

The signed-op vector preserves Glade `Op` fields 1–10 and adds B5 signature
field 11. Its signature is deliberately short test data: cryptographic validity
is supplied separately to the pure kernel, while this corpus tests structural
canonicality and byte preservation.
