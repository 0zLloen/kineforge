//! KFLOG v1 framing (DATA_SPECIFICATION.md §5.1): file header, chunk kinds,
//! chunk header, CRC. All integers little-endian.
//!
//! Chunk = 16-byte header + payload + CRC-32 + 4 zero bytes; the stride is
//! `24 + payload_len` and every chunk starts 8-byte aligned.

/// `"KFLOG\r\n\x1a"`.
pub const MAGIC: [u8; 8] = *b"KFLOG\r\n\x1a";
pub const FORMAT_VERSION: u32 = 1;
/// Magic, `u32 format_version`, `u32 reserved = 0`.
pub const FILE_HEADER: [u8; 16] = {
    let mut h = [0u8; 16];
    let mut i = 0;
    while i < 8 {
        h[i] = MAGIC[i];
        i += 1;
    }
    h[8] = FORMAT_VERSION as u8;
    h
};
pub const FILE_HEADER_LEN: u64 = 16;
pub const CHUNK_HEADER_LEN: usize = 16;
/// CRC-32 plus 4 zero bytes after the payload.
pub const CHUNK_TRAILER_LEN: usize = 8;
/// Payload limit (bytes).
pub const MAX_PAYLOAD: u32 = 1 << 20;
/// Layout version written for every v1 kind.
pub const KIND_VERSION: u8 = 1;
/// `flags` bit 0: payload LZ4-compressed (reserved; v1 writers never set it).
pub const FLAG_LZ4: u16 = 1;

/// How a kind's payload is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Packed records of this size; `payload_len == record_count × size`.
    Fixed(usize),
    /// Exactly one UTF-8 JSON object padded with spaces; `record_count == 1`.
    Json,
    /// One self-sized commit record (32 + 8n bytes); `record_count == 1`.
    Commit,
    /// Self-framed variable records; `record_count` = number of records.
    Annotation,
}

macro_rules! kinds {
    ($($var:ident = $code:literal, $name:literal, $layout:expr;)+) => {
        /// Record kinds of KFLOG v1 (DATA_SPECIFICATION.md §6).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum Kind { $($var = $code,)+ }

        impl Kind {
            pub const ALL: &'static [Kind] = &[$(Kind::$var),+];

            pub const fn from_u8(code: u8) -> Option<Kind> {
                match code { $($code => Some(Kind::$var),)+ _ => None }
            }

            /// Name used in `session_end.counts`, dumps and Parquet tables.
            pub const fn name(self) -> &'static str {
                match self { $(Kind::$var => $name,)+ }
            }

            pub const fn layout(self) -> Layout {
                match self { $(Kind::$var => $layout,)+ }
            }
        }
    };
}

kinds! {
    SessionHeader = 0x00, "session_header", Layout::Json;
    InputMotion = 0x01, "input_motion", Layout::Fixed(32);
    InputButton = 0x02, "input_button", Layout::Fixed(24);
    InputWheel = 0x03, "input_wheel", Layout::Fixed(32);
    SourceDeclare = 0x04, "source_declare", Layout::Json;
    DeviceInfo = 0x05, "device_info", Layout::Json;
    RenderState = 0x06, "render_state", Layout::Json;
    Frame = 0x10, "frame", Layout::Fixed(40);
    Drain = 0x11, "drain", Layout::Fixed(16);
    FrameTiming = 0x12, "frame_timing", Layout::Fixed(32);
    SimCamera = 0x20, "sim_camera", Layout::Fixed(40);
    SimTarget = 0x21, "sim_target", Layout::Fixed(72);
    SimShot = 0x22, "sim_shot", Layout::Fixed(56);
    SimControl = 0x23, "sim_control", Layout::Fixed(32);
    AppResult = 0x30, "app_result", Layout::Json;
    Commit = 0x7C, "commit", Layout::Commit;
    ClockSync = 0x7D, "clock_sync", Layout::Fixed(32);
    Annotation = 0x7E, "annotation", Layout::Annotation;
    SessionEnd = 0x7F, "session_end", Layout::Json;
}

impl Kind {
    /// Declaration kinds come first in a group (DATA_SPECIFICATION.md §5.2).
    pub const fn is_declaration(self) -> bool {
        matches!(
            self,
            Kind::SessionHeader | Kind::SourceDeclare | Kind::DeviceInfo
        )
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name() == name)
    }
}

/// A parsed 16-byte chunk header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkHeader {
    pub payload_len: u32,
    pub kind: u8,
    pub kind_version: u8,
    pub flags: u16,
    pub record_count: u32,
    pub reserved: u32,
}

impl ChunkHeader {
    pub fn parse(b: &[u8; CHUNK_HEADER_LEN]) -> ChunkHeader {
        ChunkHeader {
            payload_len: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            kind: b[4],
            kind_version: b[5],
            flags: u16::from_le_bytes([b[6], b[7]]),
            record_count: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            reserved: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        }
    }

    pub fn bytes(&self) -> [u8; CHUNK_HEADER_LEN] {
        let mut b = [0u8; CHUNK_HEADER_LEN];
        b[0..4].copy_from_slice(&self.payload_len.to_le_bytes());
        b[4] = self.kind;
        b[5] = self.kind_version;
        b[6..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.record_count.to_le_bytes());
        b[12..16].copy_from_slice(&self.reserved.to_le_bytes());
        b
    }
}

/// CRC-32 (IEEE, as Python `zlib.crc32`) over the chunk header and payload.
pub fn chunk_crc(header: &[u8; CHUNK_HEADER_LEN], payload: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(header);
    h.update(payload);
    h.finalize()
}

/// Appends one framed v1 chunk. `payload.len()` must be a multiple of 8 and
/// at most [`MAX_PAYLOAD`] (callers guarantee it; checked in debug builds).
pub fn write_chunk(out: &mut Vec<u8>, kind: Kind, record_count: u32, payload: &[u8]) {
    debug_assert!(payload.len().is_multiple_of(8) && payload.len() <= MAX_PAYLOAD as usize);
    let header = ChunkHeader {
        payload_len: payload.len() as u32,
        kind: kind as u8,
        kind_version: KIND_VERSION,
        flags: 0,
        record_count,
        reserved: 0,
    }
    .bytes();
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    out.extend_from_slice(&chunk_crc(&header, payload).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_spec() {
        // DATA_SPECIFICATION.md §6.1–6.5 codes, names and sizes.
        let got: Vec<(u8, &str, Layout)> = Kind::ALL
            .iter()
            .map(|&k| (k as u8, k.name(), k.layout()))
            .collect();
        use Layout::*;
        assert_eq!(
            got,
            [
                (0x00, "session_header", Json),
                (0x01, "input_motion", Fixed(32)),
                (0x02, "input_button", Fixed(24)),
                (0x03, "input_wheel", Fixed(32)),
                (0x04, "source_declare", Json),
                (0x05, "device_info", Json),
                (0x06, "render_state", Json),
                (0x10, "frame", Fixed(40)),
                (0x11, "drain", Fixed(16)),
                (0x12, "frame_timing", Fixed(32)),
                (0x20, "sim_camera", Fixed(40)),
                (0x21, "sim_target", Fixed(72)),
                (0x22, "sim_shot", Fixed(56)),
                (0x23, "sim_control", Fixed(32)),
                (0x30, "app_result", Json),
                (0x7C, "commit", Commit),
                (0x7D, "clock_sync", Fixed(32)),
                (0x7E, "annotation", Annotation),
                (0x7F, "session_end", Json),
            ]
        );
        for &k in Kind::ALL {
            assert_eq!(Kind::from_u8(k as u8), Some(k));
            assert_eq!(Kind::from_name(k.name()), Some(k));
        }
        assert_eq!(Kind::from_u8(0x07), None);
    }

    #[test]
    fn file_header_and_chunk_framing() {
        assert_eq!(
            FILE_HEADER,
            [
                0x4B, 0x46, 0x4C, 0x4F, 0x47, 0x0D, 0x0A, 0x1A, 1, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        let mut out = Vec::new();
        write_chunk(
            &mut out,
            Kind::Drain,
            1,
            &[1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0],
        );
        assert_eq!(out.len(), 24 + 16);
        let h = ChunkHeader::parse(out[..16].try_into().unwrap());
        assert_eq!(
            (
                h.payload_len,
                h.kind,
                h.kind_version,
                h.flags,
                h.record_count
            ),
            (16, 0x11, 1, 0, 1)
        );
        assert_eq!(h.bytes(), out[..16]);
        // CRC as Python zlib.crc32(header + payload).
        let crc = u32::from_le_bytes(out[32..36].try_into().unwrap());
        assert_eq!(crc, crc32fast::hash(&out[..32]));
        assert_eq!(out[36..40], [0; 4]);
        // CRC-32/IEEE check value.
        assert_eq!(crc32fast::hash(b"123456789"), 0xCBF4_3926);
    }
}
