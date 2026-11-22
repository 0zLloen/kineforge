//! Bit-exact Rust port of the Open Directional DPI engine, profile format and
//! vendor-HID packet codec (ROADMAP.md M2; OPEN_DIRECTIONAL_DPI_AUDIT.md §8).
//! `no_std`, allocation-free and dependency-free so it also builds for MCUs.
//!
//! Reference: `core/directional_dpi.{h,c}` of Open Directional DPI at commit
//! `c177d23`. Names drop the `ddpi_` prefix; struct fields keep the C names so
//! states can be compared field by field. Line ranges in doc comments refer to
//! `directional_dpi.c` unless they say `.h`. NULL-pointer branches of the C
//! API have no Rust equivalent and are omitted.
#![no_std]

mod engine;
mod hid;
mod profile;

pub use engine::{Engine, compute_scale_q16_16};
pub use profile::Profile;

/// `DDPI_PROFILE_MAGIC`: "DP", bytes `50 44` little-endian (`.h:23`).
pub const PROFILE_MAGIC: u16 = 0x4450;
/// `DDPI_PROFILE_VERSION` (`.h:24`).
pub const PROFILE_VERSION: u8 = 1;
/// `DDPI_NUM_DIRECTIONS` (`.h:25`).
pub const NUM_DIRECTIONS: usize = 8;
/// `DDPI_PROFILE_SERIALIZED_LEN` (`.h:26`).
pub const PROFILE_LEN: usize = 28;
/// `DDPI_HID_PACKET_LEN` (`.h:27`).
pub const HID_PACKET_LEN: usize = 64;
/// `DDPI_HID_NAMESPACE` (`.h:28`).
pub const HID_NAMESPACE: u8 = 0xDF;
/// `DDPI_BOOTLOADER_MAGIC_ARG`: "BLRS" (`.h:29`).
pub const BOOTLOADER_MAGIC_ARG: u32 = 0x424C_5253;

/// Q8.8 1.00x (`.h:32`).
pub const Q8_8_ONE: u16 = 256;
/// Q8.8 0.0625x (`.h:33`).
pub const Q8_8_MIN: u16 = 16;
/// Q8.8 4.00x (`.h:34`).
pub const Q8_8_MAX: u16 = 1024;
/// Q16.16 1.00x (`.h:37`).
pub const Q16_16_ONE: u32 = 65536;

/// Lowest valid base CPI (`.h:40`).
pub const DPI_MIN: u16 = 50;
/// Highest valid base CPI: the full `u16` range (`.h:41`).
pub const DPI_MAX: u16 = 65535;

/// Default travel-window length in counts, 0.5 mm at 800 CPI (`.h:44`).
pub const DEFAULT_WINDOW_COUNTS: u8 = 16;
/// Largest travel-window length in counts (`.h:45`).
pub const MAX_WINDOW_COUNTS: u8 = 128;
/// Idle gap that resets the direction estimator when the profile's idle byte
/// is 0 (`.h:48`).
pub const DEFAULT_IDLE_MS: u32 = 50;

/// `DDPI_FLAG_*` bits of [`Profile::flags`] (`.h:50-58`).
pub mod flags {
    pub const ENABLED: u8 = 1 << 0;
    /// Legacy alias with no effect: keeping the remainder is the default.
    pub const KEEP_REMAINDER_ON_REVERSE: u8 = 1 << 1;
    /// Interpolate on the tangent ratio instead of the circular angle.
    pub const LINEAR_OCTANT_RATIO: u8 = 1 << 2;
    /// Multipliers ordered counter-clockwise from +X (0=E, 1=NE, 2=N .. 7=SE).
    pub const FRAME_SENSOR_CCW: u8 = 1 << 3;
    /// Stateless 2x2 matrix model using the E/N/NE/NW gains.
    pub const MODEL_LINEAR_2X2: u8 = 1 << 4;
    /// Clear an axis remainder when the raw delta flips sign.
    pub const RESET_REMAINDER_ON_REVERSE: u8 = 1 << 5;
    /// Temporary 1.00x bypass during calibration runs.
    pub const BYPASS_FOR_CALIBRATION: u8 = 1 << 6;
    /// Apply the curve only while the sensor runs at `base_dpi`.
    pub const ONLY_AT_BASE_DPI: u8 = 1 << 7;
}

/// HID response status codes (`.h:60-64`).
pub mod status {
    pub const OK: u8 = 0xA1;
    pub const ERR_CRC_OR_FMT: u8 = 0xA2;
    pub const ERR_UNSUPPORTED: u8 = 0xA3;
    pub const ERR_RANGE: u8 = 0xA4;
}

/// Octant indices in HID coordinates, clockwise from +X with +Y toward the
/// user (`ddpi_direction_t`, `.h:66-81`).
pub mod dir {
    pub const E: u8 = 0;
    pub const SE: u8 = 1;
    pub const S: u8 = 2;
    pub const SW: u8 = 3;
    pub const W: u8 = 4;
    pub const NW: u8 = 5;
    pub const N: u8 = 6;
    pub const NE: u8 = 7;
}

/// HID protocol v1 opcodes, namespace 0xDF (`ddpi_hid_opcode_t`, `.h:95-105`).
pub mod cmd {
    pub const GET_CAPABILITIES: u8 = 0x01;
    pub const GET_PROFILE: u8 = 0x02;
    pub const SET_PROFILE: u8 = 0x03;
    pub const SAVE_FLASH: u8 = 0x04;
    pub const RESET_DEFAULTS: u8 = 0x05;
    pub const SET_ENABLED: u8 = 0x06;
    pub const SIMULATE_DELTA: u8 = 0x07;
    pub const ENTER_BOOTLOADER: u8 = 0x0F;
}

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, no
/// reflection, no final XOR. Mirrors `ddpi_crc16_ccitt` (`.c:63-76`).
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

// Little-endian helpers (`.c:16-38`). Every call site passes a constant
// offset (or 8 + 2·i with i < 8) inside a fixed-size buffer.

fn read_u16_le(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn read_u32_le(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn write_u16_le(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn write_u32_le(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
