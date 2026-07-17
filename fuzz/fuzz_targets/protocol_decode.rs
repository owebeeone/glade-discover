#![no_main]

use glade_discover_protocol::{
    decode_directory_record, decode_signed_op, decode_stream_head, decode_wire_msg,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode_signed_op(data);
    let _ = decode_directory_record(data);
    let _ = decode_stream_head(data);
    for tag in 0..=u8::MAX {
        let _ = decode_wire_msg(tag, data);
    }
});
