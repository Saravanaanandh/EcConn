//! Shared protocol types for the `lowlat-screen` workspace.
//!
//! Video is sent as one H.264 access unit per frame, split across UDP
//! datagrams. Every datagram carries the fixed 24-byte header below.
//!
//! Layout (big-endian, no padding):
//!
//! ```text
//!  0..4   magic          u32  0x4C4C5332 ("LLS2")
//!  4..5   version        u8
//!  5..6   flags          u8   bit0 keyframe, bit1 keyframe request
//!  6..10  frame_id       u32
//! 10..12  packet_index   u16
//! 12..14  packet_count   u16
//! 14..22  timestamp_us   u64
//! 22..24  payload_len    u16
//! ```

/// Capture/encode target frame rate.
pub const TARGET_FPS: u32 = 60;

/// Default port the server binds to (it also receives keyframe requests here).
pub const DEFAULT_SERVER_PORT: u16 = 9000;

/// Default port the client binds to (it receives video here).
pub const DEFAULT_CLIENT_PORT: u16 = 9100;

/// Datagram size we aim for. Safe under a 1500-byte MTU with IP+UDP headers.
pub const DATAGRAM_SIZE: usize = 1400;

pub const MAGIC: u32 = 0x4C4C_5332;
pub const VERSION: u8 = 1;

pub const FLAG_KEYFRAME: u8 = 1 << 0;
pub const FLAG_KEYFRAME_REQUEST: u8 = 1 << 1;

pub const HEADER_LEN: usize = 24;

/// Bytes of H.264 payload that fit in one datagram.
pub const MAX_PAYLOAD: usize = DATAGRAM_SIZE - HEADER_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub flags: u8,
    pub frame_id: u32,
    pub packet_index: u16,
    pub packet_count: u16,
    /// Server monotonic microseconds when the frame reached the network thread.
    /// Not comparable across machines; M4 does real latency measurement.
    pub timestamp_us: u64,
    pub payload_len: u16,
}

impl FrameHeader {
    pub fn is_keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    pub fn is_keyframe_request(&self) -> bool {
        self.flags & FLAG_KEYFRAME_REQUEST != 0
    }

    /// Write the header into the first [`HEADER_LEN`] bytes of `out`.
    pub fn encode(&self, out: &mut [u8]) {
        debug_assert!(out.len() >= HEADER_LEN);
        out[0..4].copy_from_slice(&MAGIC.to_be_bytes());
        out[4] = VERSION;
        out[5] = self.flags;
        out[6..10].copy_from_slice(&self.frame_id.to_be_bytes());
        out[10..12].copy_from_slice(&self.packet_index.to_be_bytes());
        out[12..14].copy_from_slice(&self.packet_count.to_be_bytes());
        out[14..22].copy_from_slice(&self.timestamp_us.to_be_bytes());
        out[22..24].copy_from_slice(&self.payload_len.to_be_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        if u32::from_be_bytes(buf[0..4].try_into().ok()?) != MAGIC {
            return None;
        }
        if buf[4] != VERSION {
            return None;
        }
        Some(Self {
            flags: buf[5],
            frame_id: u32::from_be_bytes(buf[6..10].try_into().ok()?),
            packet_index: u16::from_be_bytes(buf[10..12].try_into().ok()?),
            packet_count: u16::from_be_bytes(buf[12..14].try_into().ok()?),
            timestamp_us: u64::from_be_bytes(buf[14..22].try_into().ok()?),
            payload_len: u16::from_be_bytes(buf[22..24].try_into().ok()?),
        })
    }
}

/// How many datagrams a frame of `payload_bytes` needs.
pub fn packet_count(payload_bytes: usize) -> u16 {
    payload_bytes.div_ceil(MAX_PAYLOAD) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let header = FrameHeader {
            flags: FLAG_KEYFRAME,
            frame_id: 4242,
            packet_index: 3,
            packet_count: 17,
            timestamp_us: 123_456_789,
            payload_len: 1376,
        };
        let mut buf = [0u8; HEADER_LEN];
        header.encode(&mut buf);
        let decoded = FrameHeader::decode(&buf).expect("header decodes");
        assert_eq!(decoded, header);
        assert!(decoded.is_keyframe());
        assert!(!decoded.is_keyframe_request());
    }

    #[test]
    fn rejects_foreign_and_short_datagrams() {
        assert!(FrameHeader::decode(&[0u8; 4]).is_none());
        let mut buf = [0u8; HEADER_LEN];
        FrameHeader {
            flags: 0,
            frame_id: 1,
            packet_index: 0,
            packet_count: 1,
            timestamp_us: 0,
            payload_len: 0,
        }
        .encode(&mut buf);
        buf[0] = 0xFF; // break the magic
        assert!(FrameHeader::decode(&buf).is_none());
    }

    #[test]
    fn packet_counts() {
        assert_eq!(packet_count(0), 0);
        assert_eq!(packet_count(1), 1);
        assert_eq!(packet_count(MAX_PAYLOAD), 1);
        assert_eq!(packet_count(MAX_PAYLOAD + 1), 2);
        assert_eq!(packet_count(MAX_PAYLOAD * 3), 3);
    }
}
