//! Print deterministic wire vectors for the Android unit tests.
//!
//! The Android receiver must speak exactly the same UDP protocol as `common`,
//! so its tests assert against bytes produced by this crate rather than
//! hand-written expectations. Run:
//!
//! ```text
//! cargo run -p common --example wire_vectors
//! ```

use common::discovery::Beacon;
use common::pairing::{to_hex, PairMsg};
use common::{
    display_config_encode, input_msg_encode, FrameHeader, InputMsg, FLAG_KEYFRAME, HEADER_LEN,
    MAX_PAYLOAD,
};

/// Deterministic payload used on both sides (Rust here, Kotlin in the test).
fn payload(len: usize, seed: u32) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32 * 31 + seed) % 251) as u8)
        .collect()
}

/// FNV-1a 64-bit, implemented identically in the Kotlin test.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() {
    // --- a 3-packet keyframe, packets 1200 + 1200 + 600 bytes ---------------
    let frame_id: u32 = 7;
    let timestamp_us: u64 = 123_456_789;
    let payloads = vec![payload(1200, 0), payload(1200, 1), payload(600, 2)];
    let mut assembled = Vec::new();
    for (index, body) in payloads.iter().enumerate() {
        let header = FrameHeader {
            flags: FLAG_KEYFRAME,
            frame_id,
            packet_index: index as u16,
            packet_count: payloads.len() as u16,
            timestamp_us,
            payload_len: body.len() as u16,
        };
        let mut datagram = vec![0u8; HEADER_LEN + body.len()];
        header.encode(&mut datagram[..HEADER_LEN]);
        datagram[HEADER_LEN..].copy_from_slice(body);
        assembled.extend_from_slice(body);
        println!("packet{index}_header={}", to_hex(&datagram[..HEADER_LEN]));
        println!("packet{index}_fnv1a={:016x}", fnv1a(&datagram));
    }
    println!("frame_id={frame_id}");
    println!("timestamp_us={timestamp_us}");
    println!("packet_count={}", payloads.len());
    println!("assembled_len={}", assembled.len());
    println!("assembled_fnv1a={:016x}", fnv1a(&assembled));

    // --- an input MouseMove datagram (24-byte header + 17-byte payload) ------
    let seq_id: u16 = 42;
    let token: u64 = 0x1234_5678_9abc_def0;
    let mut input = [0u8; HEADER_LEN + 17];
    FrameHeader {
        flags: 0,
        frame_id: 0,
        packet_index: 0,
        packet_count: 1,
        timestamp_us: 0,
        payload_len: 17,
    }
    .encode(&mut input[..HEADER_LEN]);
    input_msg_encode(InputMsg::MouseMove, seq_id, token, &mut input[HEADER_LEN..]);
    input[HEADER_LEN + 13..HEADER_LEN + 15].copy_from_slice(&0x1234u16.to_be_bytes());
    input[HEADER_LEN + 15..HEADER_LEN + 17].copy_from_slice(&0xABCDu16.to_be_bytes());
    println!("mouse_move={}", to_hex(&input));

    // --- a DisplayConfig datagram (24-byte header + 24-byte payload) ---------
    let mut config = [0u8; HEADER_LEN + 24];
    FrameHeader {
        flags: 0,
        frame_id: 0,
        packet_index: 0,
        packet_count: 1,
        timestamp_us: 0,
        payload_len: 24,
    }
    .encode(&mut config[..HEADER_LEN]);
    display_config_encode(1920, 1080, 90, 2, 11, 0x1234, &mut config[HEADER_LEN..]);
    println!("display_config={}", to_hex(&config));

    // --- text formats --------------------------------------------------------
    let beacon = Beacon {
        id: "recv-01".into(),
        name: "Kitchen display".into(),
        platform: "android".into(),
        width: 1280,
        height: 720,
        orientation: 90,
        video_port: 9100,
        pair_port: 9201,
    };
    println!("beacon={}", beacon.encode());

    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = index as u8;
    }
    let accept = PairMsg::Accept {
        code: "123456".into(),
        token: 0xDEAD_BEEF_1234_5678,
        key,
    };
    println!("pair_accept={}", accept.encode());

    // --- sanity: MAX_PAYLOAD is what the Kotlin constants assume -------------
    assert_eq!(MAX_PAYLOAD, 1200);
}
