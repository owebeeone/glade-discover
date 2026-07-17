use std::panic::{AssertUnwindSafe, catch_unwind};

use glade_discover_protocol::{
    MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, decode_directory_record, decode_signed_op,
    decode_stream_head, decode_wire_msg, encode_directory_record, encode_stream_head,
    encode_wire_msg,
};

#[test]
fn bounded_deterministic_hostile_bytes_never_panic_any_decoder() {
    let mut random = SplitMix64::new(0x5035_D3C0_D3C0_D3C0);
    for case in 0..512_usize {
        let len = (random.next() as usize) % 2_049;
        let mut bytes = vec![0_u8; len];
        for byte in &mut bytes {
            *byte = random.next() as u8;
        }
        assert_all_decoders_do_not_panic(&bytes, case);
    }

    for (case, bytes) in [
        vec![0_u8; MAX_RECORD_BYTES + 1],
        vec![0xff_u8; MAX_RECORD_BYTES + 1],
        vec![0_u8; MAX_MESSAGE_BYTES + 1],
    ]
    .into_iter()
    .enumerate()
    {
        assert_all_decoders_do_not_panic(&bytes, 10_000 + case);
    }
}

#[test]
fn canonical_decoders_reject_truncation_trailing_and_nonminimal_mutations() {
    let signed = fixture("../../../protocol/corpus/valid/signed-op.hex");
    let record = fixture("../../../protocol/corpus/valid/capability-revocation.hex");
    let head = fixture("../../../protocol/corpus/valid/stream-head.hex");

    for end in 0..signed.len() {
        assert!(
            decode_signed_op(&signed[..end]).is_err(),
            "signed prefix {end}"
        );
    }
    for end in 0..record.len() {
        assert!(
            decode_directory_record(&record[..end]).is_err(),
            "record prefix {end}"
        );
    }
    for end in 0..head.len() {
        assert!(
            decode_stream_head(&head[..end]).is_err(),
            "head prefix {end}"
        );
    }

    for mut bytes in [signed.clone(), record.clone(), head.clone()] {
        bytes.push(0);
        assert!(decode_signed_op(&bytes).is_err());
        assert!(decode_directory_record(&bytes).is_err());
        assert!(decode_stream_head(&bytes).is_err());
    }

    assert!(decode_signed_op(&nonminimal_uint(&signed, &[0x05, 0x00])).is_err());
    assert!(decode_directory_record(&nonminimal_uint(&record, &[0x01, 0x01])).is_err());
    assert!(decode_stream_head(&nonminimal_uint(&head, &[0x05, 0x07])).is_err());

    for mut bytes in [signed, record, head] {
        bytes[0] = 0xbf;
        assert!(decode_signed_op(&bytes).is_err());
        assert!(decode_directory_record(&bytes).is_err());
        assert!(decode_stream_head(&bytes).is_err());
    }
}

#[test]
fn every_accepted_single_byte_mutation_is_still_exactly_canonical() {
    let signed = fixture("../../../protocol/corpus/valid/signed-op.hex");
    for mutated in single_byte_mutations(&signed) {
        if let Ok(op) = decode_signed_op(&mutated) {
            assert_eq!(op.canonical_bytes(), mutated);
        }
    }

    let record = fixture("../../../protocol/corpus/valid/capability-revocation.hex");
    for mutated in single_byte_mutations(&record) {
        if let Ok(decoded) = decode_directory_record(&mutated) {
            assert_eq!(
                encode_directory_record(&decoded).expect("re-encode"),
                mutated
            );
        }
    }

    let head = fixture("../../../protocol/corpus/valid/stream-head.hex");
    for mutated in single_byte_mutations(&head) {
        if let Ok(decoded) = decode_stream_head(&mutated) {
            assert_eq!(encode_stream_head(&decoded), mutated);
        }
    }

    for tag in 0..=3 {
        let canonical = match tag {
            0 => wrap_dir_op(&signed),
            1 => vec![0xa2, 0x01, 0x61, b's', 0x02, 0x80],
            2 => vec![0xa2, 0x01, 0x61, b's', 0x02, 0x80],
            3 => vec![0xa1, 0x01, 0x61, b's'],
            _ => unreachable!(),
        };
        for mutated in single_byte_mutations(&canonical) {
            if let Ok(decoded) = decode_wire_msg(tag, &mutated) {
                assert_eq!(encode_wire_msg(&decoded), (tag, mutated));
            }
        }
    }
}

fn assert_all_decoders_do_not_panic(bytes: &[u8], case: usize) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = decode_signed_op(bytes);
        let _ = decode_directory_record(bytes);
        let _ = decode_stream_head(bytes);
        for tag in 0..=5 {
            let _ = decode_wire_msg(tag, bytes);
        }
    }));
    assert!(result.is_ok(), "decoder panicked for hostile case {case}");
}

fn fixture(relative: &str) -> Vec<u8> {
    let text = match relative {
        "../../../protocol/corpus/valid/signed-op.hex" => {
            include_str!("../../../protocol/corpus/valid/signed-op.hex")
        }
        "../../../protocol/corpus/valid/capability-revocation.hex" => {
            include_str!("../../../protocol/corpus/valid/capability-revocation.hex")
        }
        "../../../protocol/corpus/valid/stream-head.hex" => {
            include_str!("../../../protocol/corpus/valid/stream-head.hex")
        }
        _ => unreachable!(),
    };
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                .expect("valid fixture hex")
        })
        .collect()
}

fn nonminimal_uint(canonical: &[u8], needle: &[u8; 2]) -> Vec<u8> {
    let at = canonical
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("target integer exists");
    let mut mutated = canonical.to_vec();
    mutated.insert(at + 1, 0x18);
    mutated
}

fn single_byte_mutations(canonical: &[u8]) -> Vec<Vec<u8>> {
    canonical
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            let mut mutated = canonical.to_vec();
            mutated[index] = byte ^ 0x01;
            mutated
        })
        .collect()
}

fn wrap_dir_op(signed: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(signed.len() + 2);
    body.extend_from_slice(&[0xa1, 0x01]);
    body.extend_from_slice(signed);
    body
}

struct SplitMix64(u64);

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}
