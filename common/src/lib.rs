// Shared protocol types for the `lowlat-screen` workspace.

// Video is sent as one H.264 access unit per frame, split across UDP
// datagrams. Every datagram carries the fixed 24-byte header below.

// ```text
//  0..4   magic          u32  0x4C4C5332 (\"LLS2\")
//  4..5   version        u8
//  5..6   flags          u8   bit0 keyframe, bit1 keyframe request
//  6..10  frame_id       u32
//  10..12  packet_index   u16
//  12..14  packet_count   u16
//  14..22  timestamp_us   u64
//  22..24  payload_len    u16
// ```
//
// Note: Input forwarding datagrams reuse this header format but carry an
// input payload described by `common::InputMsg` after the header.
//
// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```
//
// ```rust
// use common::{InputMsg, input_msg_encode, input_msg_decode};

// let mut buf = [0u8; 38]; // 24-byte header + 14-byte payload
// input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf[24..]);
// ```
//
// Redistribution of this source is permitted under the MIT license that
// governs the rest of this project.

// How many datagrams a frame of `payload_bytes` needs.
pub const MAX_PAYLOAD: usize = 1200;
pub fn packet_count(payload_bytes: usize) -> u16 {
    payload_bytes.div_ceil(MAX_PAYLOAD) as u16
}

/// Total size of a video-frame datagram (24-byte header + max payload).
pub const DATAGRAM_SIZE: usize = 24 + MAX_PAYLOAD;

// Frame header for video datagrams, exactly 24 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct FrameHeader {
    /// flags: bit0 keyframe, bit1 keyframe request
    pub flags: u8,
    /// frame_id monotonically increasing per session
    pub frame_id: u32,
    /// packet_index within the frame
    pub packet_index: u16,
    /// packet_count total packets per frame
    pub packet_count: u16,
    /// timestamp_us since epoch or stream start
    pub timestamp_us: u64,
    /// payload_len length of payload data following the header
    pub payload_len: u16,
}

impl FrameHeader {
    /// Fixed size of a frame header in bytes (24).
    pub const SIZE: usize = 24;

    /// Decode a FrameHeader from the first 24 bytes of a datagram.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            flags: data[5],
            frame_id: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
            packet_index: u16::from_be_bytes([data[10], data[11]]),
            packet_count: u16::from_be_bytes([data[12], data[13]]),
            timestamp_us: u64::from_be_bytes([
                data[14], data[15], data[16], data[17], data[18], data[19], data[20], data[21],
            ]),
            payload_len: u16::from_be_bytes([data[22], data[23]]),
        })
    }

    /// Encode this FrameHeader into the first 24 bytes of a buffer.
    pub fn encode(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::SIZE);
        buf[0..4].copy_from_slice(&0x4C4C5332u32.to_be_bytes()); // magic written once or per-packet
        buf[4] = 1; // version
        buf[5] = self.flags;
        buf[6..10].copy_from_slice(&self.frame_id.to_be_bytes());
        buf[10..12].copy_from_slice(&self.packet_index.to_be_bytes());
        buf[12..14].copy_from_slice(&self.packet_count.to_be_bytes());
        buf[14..22].copy_from_slice(&self.timestamp_us.to_be_bytes());
        buf[22..24].copy_from_slice(&self.payload_len.to_be_bytes());
    }

    /// Returns whether the keyframe flag is set.
    pub fn is_keyframe(self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    /// Returns whether the keyframe request flag is set.
    pub fn is_keyframe_request(self) -> bool {
        self.flags & FLAG_KEYFRAME_REQUEST != 0
    }
}

pub const HEADER_LEN: usize = 24;
pub const MAGIC: u32 = 0x4C4C5332;
pub const VERSION: u8 = 1;

// Default network ports
pub const DEFAULT_CLIENT_PORT: u16 = 9100;
pub const DEFAULT_SERVER_PORT: u16 = 9000;

// Maximum packets per frame for partial frame accumulation.
pub const MAX_PACKETS_PER_FRAME: u16 = 32;
// Maximum number of partial frames to keep before evicting oldest.
pub const MAX_PARTIAL_FRAMES: usize = 8;
// Keyframe request interval in milliseconds.
pub const REQUEST_INTERVAL: u64 = 1000;

// Keyframe flag bit (bit0 of flags field)
pub const FLAG_KEYFRAME: u8 = 0x01;
// Keyframe request flag bit (bit1 of flags field)
pub const FLAG_KEYFRAME_REQUEST: u8 = 0x02;

/// Input forwarding message types, sent over the existing UDP control channel
/// (server port 9000). The first byte is the message type; the rest is type-
/// specific payload. All input datagrams use the same 24-byte frame header as
// video frames, with the magic `0x4C4C5332` and version `1`, but the payload
/// data after the header carries the input message.

// Layout of the *payload* (after the 24-byte video header):

// ```text
//  0       msg_type       u8   see InputMsg below
//  1..3    reserved       u8[2]  (zero-filled for future use)
//  3..5    seq_id         u16  monotonically increasing per-session
//  5..13   session_token  u64   identifies the streaming session
// 13..    payload       varies by msg_type
// ```
//
// ```text
//  0       msg_type       u8   0x01 = MouseMove
//                               0x02 = MouseBtn
//                               0x03 = MouseWheel
//                               0x04 = Key
//                               0x05 = ReleaseAll
//  1..3    reserved       u8[2]
//  3..5    seq_id         u16
//  5..13   session_token  u64
// 13..15  mouse_x        u16  normalized x (0..65535) â€” MouseMove only
// 15..17  mouse_y        u16  normalized y (0..65535) â€” MouseMove only
// 13      button_mask      u8   bits 0=left, 1=right, 2=middle â€” MouseBtn only
// 13..15  wheel_delta    i16  positive = up/away, negative = down/toward â€” MouseWheel only
// 13      scode            u8   SDL scancode; bit 7 = key up â€” Key only
// ```

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InputMsg {
    /// Mouse moved. Normalized coordinates (u16 0..65535) relative to the
    /// displayed video area. Delta from previous position.
    MouseMove = 0x01,
    /// Mouse button state changed. Bit 0 = left, bit 1 = right, bit 2 = middle.
    MouseBtn = 0x02,
    /// Mouse wheel delta (positive = up/away, negative = down/toward).
    MouseWheel = 0x03,
    /// Key down or up. The low byte is the SDL scancode; bit 7 set means key up.
    Key = 0x04,
    /// Release all pressed keys and mouse buttons (sent on disconnect,
    /// focus loss, or 3 s timeout).
    ReleaseAll = 0x05,
    /// Client requests an orientation change (0, 90, 180, 270 degrees).
    SetOrientation = 0x06,
    /// Server notifies client of current display configuration (width, height, orientation, rotation).
    DisplayConfig = 0x07,
}

/// Encode `InputMsg` into `buf`, which must have at least 14 bytes of payload
/// space available (the full input datagram is 24-byte header + 14-byte
/// payload = 38 bytes total, but only the first 14 bytes of payload are
/// defined here; remaining bytes are ignored by the receiver).
pub fn input_msg_encode(msg: InputMsg, seq_id: u16, session_token: u64, buf: &mut [u8]) {
    debug_assert!(buf.len() >= 14);
    buf[0] = msg as u8;
    // bytes 1..3 are reserved (zero-filled)
    buf[1] = 0;
    buf[2] = 0;
    buf[3..5].copy_from_slice(&seq_id.to_be_bytes());
    buf[5..13].copy_from_slice(&session_token.to_be_bytes());
}

/// Encode a SetOrientation request into `buf` (at least 15 bytes of payload).
pub fn set_orientation_encode(orientation: u16, seq_id: u16, session_token: u64, buf: &mut [u8]) {
    debug_assert!(buf.len() >= 15);
    input_msg_encode(InputMsg::SetOrientation, seq_id, session_token, buf);
    buf[13..15].copy_from_slice(&orientation.to_be_bytes());
}

/// Decode a SetOrientation payload (requires at least 15 bytes).
pub fn set_orientation_decode(payload: &[u8]) -> Option<u16> {
    if payload.len() < 15 {
        return None;
    }
    Some(u16::from_be_bytes([payload[13], payload[14]]))
}

/// Encode a DisplayConfig notification into `buf` (at least 24 bytes of payload).
pub fn display_config_encode(
    width: u32,
    height: u32,
    orientation: u16,
    rotation: u8,
    seq_id: u16,
    session_token: u64,
    buf: &mut [u8],
) {
    debug_assert!(buf.len() >= 24);
    input_msg_encode(InputMsg::DisplayConfig, seq_id, session_token, buf);
    buf[13..17].copy_from_slice(&width.to_be_bytes());
    buf[17..21].copy_from_slice(&height.to_be_bytes());
    buf[21..23].copy_from_slice(&orientation.to_be_bytes());
    buf[23] = rotation;
}

/// Decode a DisplayConfig payload (width, height, orientation, rotation).
pub fn display_config_decode(payload: &[u8]) -> Option<(u32, u32, u16, u8)> {
    if payload.len() < 24 {
        return None;
    }
    let width = u32::from_be_bytes([payload[13], payload[14], payload[15], payload[16]]);
    let height = u32::from_be_bytes([payload[17], payload[18], payload[19], payload[20]]);
    let orientation = u16::from_be_bytes([payload[21], payload[22]]);
    let rotation = payload[23];
    Some((width, height, orientation, rotation))
}

/// Decode an input datagram's payload. Returns `None` if the payload is too
/// short for the given `msg_type`.
pub fn input_msg_decode(payload: &[u8]) -> Option<(InputMsg, u16, u64)> {
    if payload.len() < 14 {
        return None;
    }
    let msg_type = InputMsg::try_from(payload[0]).ok()?;
    let seq_id = u16::from_be_bytes(payload[3..5].try_into().ok()?);
    let session_token = u64::from_be_bytes(payload[5..13].try_into().ok()?);
    Some((msg_type, seq_id, session_token))
}

impl TryFrom<u8> for InputMsg {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(InputMsg::MouseMove),
            0x02 => Ok(InputMsg::MouseBtn),
            0x03 => Ok(InputMsg::MouseWheel),
            0x04 => Ok(InputMsg::Key),
            0x05 => Ok(InputMsg::ReleaseAll),
            0x06 => Ok(InputMsg::SetOrientation),
            0x07 => Ok(InputMsg::DisplayConfig),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_msg_roundtrip() {
        let mut buf = [0u8; 14];
        input_msg_encode(InputMsg::MouseMove, 42, 0x123456789abcdef0, &mut buf);
        let (msg, seq, token) = input_msg_decode(&buf).expect("input msg decodes");
        assert_eq!(msg, InputMsg::MouseMove);
        assert_eq!(seq, 42);
        assert_eq!(token, 0x123456789abcdef0);
    }

    #[test]
    fn input_msg_rejects_too_short() {
        let buf = [0u8; 10];
        assert!(input_msg_decode(&buf).is_none());
    }

    #[test]
    fn input_msg_try_from() {
        assert_eq!(InputMsg::try_from(0x01), Ok(InputMsg::MouseMove));
        assert_eq!(InputMsg::try_from(0x05), Ok(InputMsg::ReleaseAll));
        assert_eq!(InputMsg::try_from(0x06), Ok(InputMsg::SetOrientation));
        assert_eq!(InputMsg::try_from(0x07), Ok(InputMsg::DisplayConfig));
        assert_eq!(InputMsg::try_from(0xFF), Err(()));
    }

    #[test]
    fn orientation_messages_roundtrip() {
        let mut buf = [0u8; 32];
        set_orientation_encode(90, 10, 0x1234, &mut buf);
        let (msg, seq, token) = input_msg_decode(&buf).unwrap();
        assert_eq!(msg, InputMsg::SetOrientation);
        assert_eq!(seq, 10);
        assert_eq!(token, 0x1234);
        assert_eq!(set_orientation_decode(&buf), Some(90));

        let mut buf2 = [0u8; 32];
        display_config_encode(720, 1280, 90, 2, 11, 0x1234, &mut buf2);
        let (msg2, seq2, token2) = input_msg_decode(&buf2).unwrap();
        assert_eq!(msg2, InputMsg::DisplayConfig);
        assert_eq!(seq2, 11);
        assert_eq!(token2, 0x1234);
        let (w, h, orient, rot) = display_config_decode(&buf2).unwrap();
        assert_eq!(w, 720);
        assert_eq!(h, 1280);
        assert_eq!(orient, 90);
        assert_eq!(rot, 2);
    }
}
