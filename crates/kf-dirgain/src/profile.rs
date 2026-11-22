//! The 28-byte v1 profile: defaults, sanitising, CPI headroom, frame
//! conversion and the little-endian wire format (`.c:78-266`, `.h:107-170`).

use crate::{
    DEFAULT_IDLE_MS, DPI_MAX, DPI_MIN, MAX_WINDOW_COUNTS, NUM_DIRECTIONS, PROFILE_LEN,
    PROFILE_MAGIC, PROFILE_VERSION, Q8_8_MAX, Q8_8_MIN, Q8_8_ONE, crc16_ccitt, flags, read_u16_le,
    write_u16_le,
};

/// Versioned 28-byte directional DPI profile (`ddpi_profile_v1_t`,
/// `.h:107-123`). Multipliers and gain are Q8.8 (256 = 1.00x).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// +0x00: [`PROFILE_MAGIC`].
    pub magic: u16,
    /// +0x02: [`PROFILE_VERSION`].
    pub version: u8,
    /// +0x03: [`flags`] bitmask.
    pub flags: u8,
    /// +0x04: base sensor CPI (50..=65535).
    pub base_dpi: u16,
    /// +0x06: global gain.
    pub global_gain_q8_8: u16,
    /// +0x08: one multiplier per octant, indexed by [`crate::dir`] (or
    /// counter-clockwise with [`flags::FRAME_SENSOR_CCW`]).
    pub multipliers_q8_8: [u16; NUM_DIRECTIONS],
    /// +0x18: low byte = travel-window length L in counts (0 = instantaneous),
    /// high byte = idle reset timeout in 5 ms units (0 = [`DEFAULT_IDLE_MS`]).
    pub reserved: u16,
    /// +0x1A: CRC-16-CCITT over bytes 0..26.
    pub crc16: u16,
}

/// `clamp_q8_8` (`.c:40-48`).
fn clamp_q8_8(val: u16) -> u16 {
    val.clamp(Q8_8_MIN, Q8_8_MAX)
}

/// `clamp_dpi` (`.c:50-53`); the upper bound is the full `u16` range.
fn clamp_dpi(dpi: u16) -> u16 {
    dpi.max(DPI_MIN)
}

impl Profile {
    /// Neutral 1.00x profile, disabled; `base_dpi` 0 means 800.
    /// Mirrors `ddpi_profile_init_default` (`.c:78-93`).
    pub fn new_default(base_dpi: u16) -> Self {
        let mut profile = Profile {
            magic: PROFILE_MAGIC,
            version: PROFILE_VERSION,
            flags: 0,
            base_dpi: clamp_dpi(if base_dpi == 0 { 800 } else { base_dpi }),
            global_gain_q8_8: Q8_8_ONE,
            multipliers_q8_8: [Q8_8_ONE; NUM_DIRECTIONS],
            reserved: 0,
            crc16: 0,
        };
        profile.sanitize_and_sign();
        profile
    }

    /// Travel-window length L in counts, capped at 128.
    /// Mirrors `ddpi_profile_get_window_counts` (`.h:145-151`).
    pub fn window_counts(&self) -> u8 {
        ((self.reserved & 0xFF) as u8).min(MAX_WINDOW_COUNTS)
    }

    /// Mirrors `ddpi_profile_set_window_counts` (`.h:153-161`); values above
    /// 128 are clamped. Does not re-sign.
    pub fn set_window_counts(&mut self, window_counts: u8) {
        let w = window_counts.min(MAX_WINDOW_COUNTS);
        self.reserved = (self.reserved & 0xFF00) | u16::from(w);
    }

    /// Idle gap in ms after which the direction estimator resets.
    /// Mirrors `ddpi_profile_get_idle_ms` (`.h:167-170`).
    pub fn idle_ms(&self) -> u32 {
        let units = u32::from(self.reserved >> 8);
        if units == 0 {
            DEFAULT_IDLE_MS
        } else {
            units * 5
        }
    }

    /// Clamps every field to its v1 bounds and recomputes `crc16`.
    /// Mirrors `ddpi_profile_sanitize_and_sign` (`.c:95-123`).
    pub fn sanitize_and_sign(&mut self) {
        self.magic = PROFILE_MAGIC;
        self.version = PROFILE_VERSION;
        self.base_dpi = clamp_dpi(self.base_dpi);
        self.global_gain_q8_8 = clamp_q8_8(self.global_gain_q8_8);
        for m in &mut self.multipliers_q8_8 {
            *m = clamp_q8_8(*m);
        }
        self.set_window_counts(self.window_counts());
        self.crc16 = crc16_ccitt(&self.raw_bytes()[..26]);
    }

    /// CPI headroom: if the largest effective multiplier `max(M[i]·G)` is
    /// above 1.00x, rescales the curve so it becomes exactly 1.00x for any
    /// sanitised profile (an unsanitised `M·G >= 2^24` wraps, as in the C; global
    /// gain becomes 1.00x) and returns the sensor CPI to run at,
    /// `base_dpi · max(M·G)` rounded and clamped to 50..=65535, so firmware
    /// only scales down. If it is 1.00x or less, nothing is rescaled (gain
    /// unchanged): the profile is only sanitised and re-signed, and the
    /// return value is `base_dpi` as stored before sanitising, like the C.
    /// Mirrors `ddpi_profile_normalize_cpi_headroom` (`.c:125-164`); the
    /// return value is `*out_sensor_cpi`.
    pub fn normalize_cpi_headroom(&mut self) -> u16 {
        let gain = u32::from(self.global_gain_q8_8);
        // u16 · u16 always fits u32.
        let max_eff_q16 = self
            .multipliers_q8_8
            .iter()
            .map(|&m| u32::from(m) * gain)
            .fold(0, u32::max);

        if max_eff_q16 <= 65536 {
            let sensor_cpi = self.base_dpi;
            self.sanitize_and_sign();
            return sensor_cpi;
        }

        let boosted_cpi = (u64::from(self.base_dpi) * u64::from(max_eff_q16) + 32768) >> 16;
        let sensor_cpi = clamp_dpi(boosted_cpi.min(u64::from(DPI_MAX)) as u16);

        for m in &mut self.multipliers_q8_8 {
            let eff = u32::from(*m) * gain;
            // u32 arithmetic like the C; it wraps only for unsanitised M·G >= 2^24.
            let norm_q8 = eff.wrapping_mul(256).wrapping_add(max_eff_q16 >> 1) / max_eff_q16;
            *m = clamp_q8_8(norm_q8.min(u32::from(Q8_8_ONE)) as u16);
        }
        self.global_gain_q8_8 = Q8_8_ONE;
        self.sanitize_and_sign();
        sensor_cpi
    }

    /// Converts the multipliers in place between HID clockwise order and
    /// `odd-sensor-ccw` order (swaps i <-> 8 - i), toggles
    /// [`flags::FRAME_SENSOR_CCW`] and re-signs. No-op if already in that
    /// frame. Mirrors `ddpi_profile_set_ccw_frame` (`.c:166-187`).
    pub fn set_ccw_frame(&mut self, use_ccw: bool) {
        let currently_ccw = self.flags & flags::FRAME_SENSOR_CCW != 0;
        if currently_ccw == use_ccw {
            return;
        }
        for i in 1..4 {
            self.multipliers_q8_8.swap(i, 8 - i);
        }
        if use_ccw {
            self.flags |= flags::FRAME_SENSOR_CCW;
        } else {
            self.flags &= !flags::FRAME_SENSOR_CCW;
        }
        self.sanitize_and_sign();
    }

    /// Sanitised, signed 28-byte little-endian image.
    /// Mirrors `ddpi_profile_serialize` (`.c:189-206`).
    pub fn serialize(&self) -> [u8; PROFILE_LEN] {
        let mut clean = *self;
        clean.sanitize_and_sign();
        clean.raw_bytes()
    }

    /// Parses a 28-byte image; `None` unless magic, version, CRC, `base_dpi`
    /// and every Q8.8 value are valid. `flags` and `reserved` are taken as
    /// they are. Mirrors `ddpi_profile_deserialize` (`.c:208-247`).
    pub fn deserialize(buf: &[u8; PROFILE_LEN]) -> Option<Profile> {
        let magic = read_u16_le(buf, 0);
        let version = buf[2];
        if magic != PROFILE_MAGIC || version != PROFILE_VERSION {
            return None;
        }

        let crc16 = read_u16_le(buf, 26);
        if crc16_ccitt(&buf[..26]) != crc16 {
            return None;
        }

        let base_dpi = read_u16_le(buf, 4);
        let global_gain_q8_8 = read_u16_le(buf, 6);
        if base_dpi < DPI_MIN || !(Q8_8_MIN..=Q8_8_MAX).contains(&global_gain_q8_8) {
            return None;
        }

        let mut multipliers_q8_8 = [0; NUM_DIRECTIONS];
        for (i, m) in multipliers_q8_8.iter_mut().enumerate() {
            *m = read_u16_le(buf, 8 + i * 2);
            if !(Q8_8_MIN..=Q8_8_MAX).contains(m) {
                return None;
            }
        }

        Some(Profile {
            magic,
            version,
            flags: buf[3],
            base_dpi,
            global_gain_q8_8,
            multipliers_q8_8,
            reserved: read_u16_le(buf, 24),
            crc16,
        })
    }

    /// True when the curve is an exact 1.00x pass-through: disabled, in
    /// calibration bypass, or all gains 1.00x.
    /// Mirrors `ddpi_profile_is_identity` (`.c:249-266`).
    pub fn is_identity(&self) -> bool {
        if self.flags & flags::ENABLED == 0 || self.flags & flags::BYPASS_FOR_CALIBRATION != 0 {
            return true;
        }
        self.global_gain_q8_8 == Q8_8_ONE && self.multipliers_q8_8.iter().all(|&m| m == Q8_8_ONE)
    }

    /// Multiplier for a HID clockwise octant index, honouring
    /// [`flags::FRAME_SENSOR_CCW`]. Mirrors `get_sector_multiplier_q8_8`
    /// (`.c:338-345`).
    pub(crate) fn sector_multiplier(&self, hid_cw_idx: u8) -> u16 {
        let mut idx = hid_cw_idx & 7;
        if self.flags & flags::FRAME_SENSOR_CCW != 0 {
            idx = (8 - idx) & 7;
        }
        self.multipliers_q8_8[usize::from(idx)]
    }

    /// Little-endian image of the fields as stored (no sanitising), with the
    /// stored `crc16` in bytes 26..28. Shared by `.c:112-122` and `.c:196-205`.
    fn raw_bytes(&self) -> [u8; PROFILE_LEN] {
        let mut buf = [0; PROFILE_LEN];
        write_u16_le(&mut buf, 0, self.magic);
        buf[2] = self.version;
        buf[3] = self.flags;
        write_u16_le(&mut buf, 4, self.base_dpi);
        write_u16_le(&mut buf, 6, self.global_gain_q8_8);
        for (i, &m) in self.multipliers_q8_8.iter().enumerate() {
            write_u16_le(&mut buf, 8 + i * 2, m);
        }
        write_u16_le(&mut buf, 24, self.reserved);
        write_u16_le(&mut buf, 26, self.crc16);
        buf
    }
}
