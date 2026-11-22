//! Dev-only oracle for `kf-dirgain` (ROADMAP.md M2; OPEN_DIRECTIONAL_DPI_AUDIT.md
//! §8): the vendored Open Directional DPI C engine ([`c`]), engine operations
//! that run on either implementation ([`Op`]), and a seeded generator of
//! profiles, deltas and operations ([`XorShift`]). Used by `gen-traces` and
//! `tests/fuzz.rs`; see README.md.

pub mod c;

pub use c::{COracle, HidCalls};
use kf_dirgain::{
    BOOTLOADER_MAGIC_ARG, Engine, HID_NAMESPACE, HID_PACKET_LEN, PROFILE_LEN, PROFILE_MAGIC,
    PROFILE_VERSION, Profile, cmd, flags,
};

/// Byte every response buffer holds before a request, so a request that is
/// not handled visibly leaves it untouched.
pub const RESPONSE_FILL: u8 = 0xEE;

/// Little-endian image of the fields as stored: no sanitising, the stored
/// `crc16` (the C struct's in-memory image on a little-endian machine).
pub fn profile_to_raw(p: &Profile) -> [u8; PROFILE_LEN] {
    let mut b = [0; PROFILE_LEN];
    b[0..2].copy_from_slice(&p.magic.to_le_bytes());
    b[2] = p.version;
    b[3] = p.flags;
    b[4..6].copy_from_slice(&p.base_dpi.to_le_bytes());
    b[6..8].copy_from_slice(&p.global_gain_q8_8.to_le_bytes());
    for (i, m) in p.multipliers_q8_8.iter().enumerate() {
        b[8 + 2 * i..10 + 2 * i].copy_from_slice(&m.to_le_bytes());
    }
    b[24..26].copy_from_slice(&p.reserved.to_le_bytes());
    b[26..28].copy_from_slice(&p.crc16.to_le_bytes());
    b
}

/// Inverse of [`profile_to_raw`]; accepts any bytes.
pub fn profile_from_raw(b: &[u8; PROFILE_LEN]) -> Profile {
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    Profile {
        magic: u16_at(0),
        version: b[2],
        flags: b[3],
        base_dpi: u16_at(4),
        global_gain_q8_8: u16_at(6),
        multipliers_q8_8: std::array::from_fn(|i| u16_at(8 + 2 * i)),
        reserved: u16_at(24),
        crc16: u16_at(26),
    }
}

/// [`profile_to_raw`] with a CRC that matches bytes 0..26, so `deserialize`
/// gets past the CRC check and tests the field bounds.
pub fn signed_raw(p: &Profile) -> [u8; PROFILE_LEN] {
    let mut b = profile_to_raw(p);
    let crc = c::crc16_ccitt(&b[..26]);
    b[26..28].copy_from_slice(&crc.to_le_bytes());
    b
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A well-formed HID v1 request: `'D','P'`, version, status 0, `seq`, 0xDF,
/// `opcode`, `slot`, `payload` (up to 54 bytes), CRC-16 LE over bytes 0..62.
pub fn hid_request(seq: u8, opcode: u8, slot: u8, payload: &[u8]) -> [u8; HID_PACKET_LEN] {
    let mut req = [0; HID_PACKET_LEN];
    req[..8].copy_from_slice(&[
        0x44,
        0x50,
        PROFILE_VERSION,
        0,
        seq,
        HID_NAMESPACE,
        opcode,
        slot,
    ]);
    req[8..8 + payload.len()].copy_from_slice(payload);
    sign_request(&mut req);
    req
}

/// Recomputes the request CRC (bytes 62..64).
pub fn sign_request(req: &mut [u8; HID_PACKET_LEN]) {
    let crc = c::crc16_ccitt(&req[..62]);
    req[62..].copy_from_slice(&crc.to_le_bytes());
}

/// One engine operation, runnable on the C engine and on the Rust port.
/// `Normalize`, `Ccw` and `Window` act on the engine's profile in place.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Delta(i16, i16),
    Motion(i16, i16, u32),
    SensorDpi(u16),
    Clock(u32),
    SetProfile(Profile),
    NotifyIdle,
    ResetRemainders,
    Normalize,
    Ccw(bool),
    Window(u8),
    /// `save`: `None` = no save callback, else what it returns.
    Hid {
        req: [u8; HID_PACKET_LEN],
        save: Option<bool>,
        boot: bool,
    },
}

/// What an [`Op`] returns besides the engine state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Delta(i16, i16),
    SensorCpi(u16),
    Hid {
        handled: bool,
        resp: [u8; HID_PACKET_LEN],
        calls: HidCalls,
    },
}

impl Op {
    pub fn run_c(&self, e: &mut COracle) -> Outcome {
        match *self {
            Op::Delta(dx, dy) => {
                let (x, y) = e.process_delta(dx, dy);
                Outcome::Delta(x, y)
            }
            Op::Motion(dx, dy, now) => {
                let (x, y) = e.process_motion(dx, dy, now);
                Outcome::Delta(x, y)
            }
            Op::SensorDpi(dpi) => {
                e.set_sensor_dpi(dpi);
                Outcome::Nothing
            }
            Op::Clock(now) => {
                e.set_clock_ms(now);
                Outcome::Nothing
            }
            Op::SetProfile(p) => {
                e.set_profile(&p);
                Outcome::Nothing
            }
            Op::NotifyIdle => {
                e.notify_idle();
                Outcome::Nothing
            }
            Op::ResetRemainders => {
                e.reset_remainders();
                Outcome::Nothing
            }
            Op::Normalize => Outcome::SensorCpi(e.normalize_cpi_headroom()),
            Op::Ccw(use_ccw) => {
                e.set_ccw_frame(use_ccw);
                Outcome::Nothing
            }
            Op::Window(w) => {
                e.set_window_counts(w);
                Outcome::Nothing
            }
            Op::Hid { req, save, boot } => {
                let mut resp = [RESPONSE_FILL; HID_PACKET_LEN];
                let (handled, calls) = e.handle_feature_report(&req, &mut resp, save, boot);
                Outcome::Hid {
                    handled,
                    resp,
                    calls,
                }
            }
        }
    }

    pub fn run_rust(&self, e: &mut Engine) -> Outcome {
        match *self {
            Op::Delta(dx, dy) => {
                let (x, y) = e.process_delta(dx, dy);
                Outcome::Delta(x, y)
            }
            Op::Motion(dx, dy, now) => {
                let (x, y) = e.process_motion(dx, dy, now);
                Outcome::Delta(x, y)
            }
            Op::SensorDpi(dpi) => {
                e.set_sensor_dpi(dpi);
                Outcome::Nothing
            }
            Op::Clock(now) => {
                e.set_clock_ms(now);
                Outcome::Nothing
            }
            Op::SetProfile(p) => {
                e.set_profile(&p);
                Outcome::Nothing
            }
            Op::NotifyIdle => {
                e.notify_idle();
                Outcome::Nothing
            }
            Op::ResetRemainders => {
                e.reset_remainders();
                Outcome::Nothing
            }
            Op::Normalize => Outcome::SensorCpi(e.profile.normalize_cpi_headroom()),
            Op::Ccw(use_ccw) => {
                e.profile.set_ccw_frame(use_ccw);
                Outcome::Nothing
            }
            Op::Window(w) => {
                e.profile.set_window_counts(w);
                Outcome::Nothing
            }
            Op::Hid { req, save, boot } => {
                let mut resp = [RESPONSE_FILL; HID_PACKET_LEN];
                let mut calls = HidCalls::default();
                let mut boot_calls = 0;
                let mut save_cb = |p: &Profile| {
                    calls.save_calls += 1;
                    calls.saved_profile = Some(*p);
                    save.unwrap_or(false)
                };
                let mut boot_cb = || boot_calls += 1;
                let save_arg: Option<&mut dyn FnMut(&Profile) -> bool> = if save.is_some() {
                    Some(&mut save_cb)
                } else {
                    None
                };
                let boot_arg: Option<&mut dyn FnMut()> =
                    if boot { Some(&mut boot_cb) } else { None };
                let handled = e.handle_feature_report(&req, &mut resp, save_arg, boot_arg);
                calls.boot_calls = boot_calls;
                Outcome::Hid {
                    handled,
                    resp,
                    calls,
                }
            }
        }
    }
}

/// xorshift64 with biased generators for engine inputs. Deterministic for a
/// given seed on every platform.
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // Odd multiplier keeps distinct seeds distinct; `| 1` avoids the zero state.
        XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-enough value in `0..n` (modulo bias is irrelevant here).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) as u32) % n
    }

    pub fn u8(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    pub fn u16(&mut self) -> u16 {
        (self.next_u64() >> 48) as u16
    }

    pub fn u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub fn bool(&mut self) -> bool {
        self.below(2) == 0
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }

    /// Sensor delta biased toward boundaries: int16 extremes, octant edges,
    /// estimator-clamp edges and small steps.
    pub fn delta(&mut self) -> i16 {
        const EDGES: [i16; 15] = [
            i16::MIN,
            -32767,
            -32766,
            -16384,
            -129,
            -128,
            -1,
            0,
            1,
            127,
            128,
            16383,
            32766,
            i16::MAX,
            64,
        ];
        match self.below(8) {
            0 | 1 => self.pick(&EDGES),
            2 | 3 => self.below(7) as i16 - 3,
            4 => self.below(81) as i16 - 40,
            5 => self.below(2001) as i16 - 1000,
            _ => self.u16() as i16,
        }
    }

    /// Q8.8 value, mostly in 16..=1024 with both extremes, sometimes outside
    /// (set_profile clamps those).
    pub fn q8(&mut self) -> u16 {
        match self.below(10) {
            0 => 16,
            1 => 1024,
            2 | 3 => 256,
            4 => self.pick(&[0, 15, 17, 255, 257, 1023, 1025, u16::MAX]),
            _ => 16 + self.below(1009) as u16,
        }
    }

    /// Profile with every field random but biased: flag bytes of all kinds
    /// (mostly enabled), identity curves, window/idle edge values, base CPI
    /// incl. 0/49/50/65535, and occasionally a wrong magic and/or version.
    pub fn profile(&mut self) -> Profile {
        let flags = match self.below(8) {
            0 => self.u8(),
            1 => self.u8() | flags::ENABLED,
            _ => (self.u8() | flags::ENABLED) & !flags::BYPASS_FOR_CALIBRATION,
        };
        let identity = self.below(10) == 0;
        let mut multipliers_q8_8 = [256; 8];
        let mut global_gain_q8_8 = 256;
        if !identity {
            for m in &mut multipliers_q8_8 {
                *m = self.q8();
            }
            global_gain_q8_8 = self.q8();
        }
        let any_dpi = self.u16();
        let base_dpi = self.pick(&[0, 49, 50, 400, 800, 1600, 3200, 26000, 65535, any_dpi]);
        let any = self.u8();
        let window = self.pick(&[0, 0, 1, 16, 31, 32, 64, 128, 129, 255, any, any]);
        let any = self.u8();
        let idle = self.pick(&[0, 0, 0, 1, 2, 10, 255, any]);
        // Drawn independently, so signed buffers with the right magic and a
        // wrong version (and vice versa) reach deserialize's version check.
        let magic = if self.below(16) == 0 {
            self.u16()
        } else {
            PROFILE_MAGIC
        };
        let version = if self.below(16) == 0 {
            self.u8()
        } else {
            PROFILE_VERSION
        };
        Profile {
            magic,
            version,
            flags,
            base_dpi,
            global_gain_q8_8,
            multipliers_q8_8,
            reserved: u16::from(idle) << 8 | u16::from(window),
            crc16: self.u16(),
        }
    }

    /// Timestamp for the next motion: mostly a few ms after the last motion,
    /// often exactly at idle - 1 / idle / idle + 1, sometimes anywhere (which
    /// also crosses the u32 wrap).
    pub fn now(&mut self, e: &Engine) -> u32 {
        let idle = e.profile.idle_ms();
        let dt = match self.below(10) {
            0..=4 => self.below(4),
            5 => idle - 1,
            6 => idle,
            7 => idle + 1,
            _ => self.u32(),
        };
        e.last_motion_ms.wrapping_add(dt)
    }

    /// A random engine operation for the engine state `e`.
    pub fn op(&mut self, e: &Engine) -> Op {
        match self.below(100) {
            0..=39 => {
                let (dx, dy) = self.delta_pair();
                Op::Motion(dx, dy, self.now(e))
            }
            40..=64 => {
                let (dx, dy) = self.delta_pair();
                Op::Delta(dx, dy)
            }
            65..=69 => {
                let any = self.u16();
                Op::SensorDpi(self.pick(&[0, e.profile.base_dpi, e.profile.base_dpi, 1600, any]))
            }
            70 => Op::Clock(self.u32()),
            71..=76 => Op::SetProfile(self.profile()),
            77 => Op::NotifyIdle,
            78..=79 => Op::ResetRemainders,
            80 => Op::Normalize,
            81 => Op::Ccw(self.bool()),
            82 => {
                let any = self.u8();
                Op::Window(self.pick(&[0, 1, 16, 128, 129, any]))
            }
            _ => self.hid(),
        }
    }

    /// (dx, dy) with one axis often zero (octant edges) and (0, 0) sometimes.
    fn delta_pair(&mut self) -> (i16, i16) {
        match self.below(10) {
            0 => (0, 0),
            1 => (self.delta(), 0),
            2 => (0, self.delta()),
            _ => (self.delta(), self.delta()),
        }
    }

    /// A HID request: mostly well-formed with a meaningful payload for its
    /// opcode, sometimes random payload bytes, an unknown opcode, broken
    /// framing, a wrong version or a bad CRC.
    pub fn hid(&mut self) -> Op {
        let opcode = if self.below(10) == 0 {
            self.u8()
        } else {
            self.pick(&[
                cmd::GET_CAPABILITIES,
                cmd::GET_PROFILE,
                cmd::SET_PROFILE,
                cmd::SET_PROFILE,
                cmd::SAVE_FLASH,
                cmd::RESET_DEFAULTS,
                cmd::SET_ENABLED,
                cmd::SIMULATE_DELTA,
                cmd::SIMULATE_DELTA,
                cmd::ENTER_BOOTLOADER,
            ])
        };
        let mut payload = [0u8; 54];
        if self.below(5) == 0 {
            payload.fill_with(|| self.u8());
        }
        match opcode {
            cmd::SET_PROFILE => {
                let p = self.profile();
                let mut bytes = match self.below(4) {
                    0 => signed_raw(&p),
                    _ => c::serialize(&p),
                };
                if self.below(5) == 0 {
                    bytes[self.below(28) as usize] ^= 1 << self.below(8);
                }
                payload[..PROFILE_LEN].copy_from_slice(&bytes);
            }
            cmd::SET_ENABLED => {
                let any = self.u8();
                payload[0] = self.pick(&[0, 1, 2, 3, 255, any]);
            }
            cmd::SIMULATE_DELTA => {
                let (dx, dy) = self.delta_pair();
                payload[0..2].copy_from_slice(&dx.to_le_bytes());
                payload[2..4].copy_from_slice(&dy.to_le_bytes());
            }
            cmd::ENTER_BOOTLOADER if self.bool() => {
                payload[0..4].copy_from_slice(&BOOTLOADER_MAGIC_ARG.to_le_bytes());
            }
            _ => {}
        }
        let (seq, slot) = (self.u8(), self.u8());
        let mut req = hid_request(seq, opcode, slot, &payload);
        match self.below(20) {
            0 => req[self.below(2) as usize] ^= 1 << self.below(8),
            1 => req[5] ^= 1 << self.below(8),
            2 => {
                req[2] = self.pick(&[0, 2, 0xFF]);
                sign_request(&mut req);
            }
            3 => req[62 + self.below(2) as usize] ^= 1 << self.below(8),
            4 => req[self.below(64) as usize] ^= 1 << self.below(8),
            _ => {}
        }
        let save = self.pick(&[None, Some(true), Some(false)]);
        Op::Hid {
            req,
            save,
            boot: self.bool(),
        }
    }
}
