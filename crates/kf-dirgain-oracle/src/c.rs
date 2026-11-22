//! The vendored C engine (`c/directional_dpi.c`): `#[repr(C)]` mirrors of its
//! structs, the `extern "C"` declarations and safe wrappers over
//! [`kf_dirgain::Profile`] / [`kf_dirgain::Engine`]. The only module of this
//! crate that may use `unsafe`.
#![allow(unsafe_code)]

use std::cell::Cell;

use kf_dirgain::{Engine, HID_PACKET_LEN, NUM_DIRECTIONS, PROFILE_LEN, Profile};

/// `ddpi_profile_v1_t` (`directional_dpi.h:107-123`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CProfile {
    magic: u16,
    version: u8,
    flags: u8,
    base_dpi: u16,
    global_gain_q8_8: u16,
    multipliers_q8_8: [u16; NUM_DIRECTIONS],
    reserved: u16,
    crc16: u16,
}

/// `ddpi_engine_t` (`directional_dpi.h:125-140`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CEngine {
    profile: CProfile,
    rem_x_q16_16: i32,
    rem_y_q16_16: i32,
    dir_x_q16_16: i32,
    dir_y_q16_16: i32,
    last_octant: u8,
    last_scale_q16_16: u32,
    last_motion_ms: u32,
    sensor_dpi: u16,
    clock_known: u8,
    clock_ms: u32,
}

// SAFETY: the declarations match `c/directional_dpi.h` and `c/kf_shim.c`; the
// `repr_c_mirrors_match_c_layout` test checks both struct mirrors against the
// C compiler's sizes and offsets. Items marked `safe` take only integers and
// references (non-null, aligned, initialised, and for arrays exactly the length
// the C indexes), have no other preconditions and keep no pointer after
// returning. A callback receives `&engine->profile` of the engine passed in,
// valid for the duration of the callback.
unsafe extern "C" {
    fn ddpi_crc16_ccitt(data: *const u8, len: usize) -> u16;
    safe fn ddpi_profile_init_default(profile: &mut CProfile, base_dpi: u16);
    safe fn ddpi_profile_sanitize_and_sign(profile: &mut CProfile);
    safe fn ddpi_profile_normalize_cpi_headroom(profile: &mut CProfile, out_sensor_cpi: &mut u16);
    safe fn ddpi_profile_set_ccw_frame(profile: &mut CProfile, use_ccw_frame: bool);
    safe fn ddpi_profile_serialize(profile: &CProfile, out_buf: &mut [u8; PROFILE_LEN]);
    safe fn ddpi_profile_deserialize(
        in_buf: &[u8; PROFILE_LEN],
        out_profile: &mut CProfile,
    ) -> bool;
    safe fn ddpi_profile_is_identity(profile: &CProfile) -> bool;
    safe fn ddpi_engine_init(engine: &mut CEngine, base_dpi: u16);
    safe fn ddpi_engine_reset_remainders(engine: &mut CEngine);
    safe fn ddpi_engine_notify_idle(engine: &mut CEngine);
    safe fn ddpi_engine_set_sensor_dpi(engine: &mut CEngine, sensor_dpi: u16);
    safe fn ddpi_engine_set_clock_ms(engine: &mut CEngine, now_ms: u32);
    safe fn ddpi_engine_set_profile(engine: &mut CEngine, profile: &CProfile);
    safe fn ddpi_compute_scale_q16_16(
        profile: &CProfile,
        dx: i16,
        dy: i16,
        out_octant: &mut u8,
    ) -> u32;
    safe fn ddpi_process_delta(
        engine: &mut CEngine,
        in_dx: i16,
        in_dy: i16,
        out_dx: &mut i16,
        out_dy: &mut i16,
    );
    safe fn ddpi_process_motion(
        engine: &mut CEngine,
        in_dx: i16,
        in_dy: i16,
        now_ms: u32,
        out_dx: &mut i16,
        out_dy: &mut i16,
    );
    safe fn ddpi_hid_handle_feature_report(
        engine: &mut CEngine,
        req: &[u8; HID_PACKET_LEN],
        resp: &mut [u8; HID_PACKET_LEN],
        on_save_flash: Option<extern "C" fn(&CProfile) -> bool>,
        on_enter_bootloader: Option<extern "C" fn()>,
    ) -> bool;
    #[cfg(test)]
    safe fn kf_shim_layout(i: usize) -> usize;
    safe fn kf_shim_get_window_counts(profile: &CProfile) -> u8;
    safe fn kf_shim_set_window_counts(profile: &mut CProfile, window_counts: u8);
    safe fn kf_shim_get_idle_ms(profile: &CProfile) -> u32;
}

fn to_c(p: &Profile) -> CProfile {
    CProfile {
        magic: p.magic,
        version: p.version,
        flags: p.flags,
        base_dpi: p.base_dpi,
        global_gain_q8_8: p.global_gain_q8_8,
        multipliers_q8_8: p.multipliers_q8_8,
        reserved: p.reserved,
        crc16: p.crc16,
    }
}

fn from_c(p: &CProfile) -> Profile {
    Profile {
        magic: p.magic,
        version: p.version,
        flags: p.flags,
        base_dpi: p.base_dpi,
        global_gain_q8_8: p.global_gain_q8_8,
        multipliers_q8_8: p.multipliers_q8_8,
        reserved: p.reserved,
        crc16: p.crc16,
    }
}

/// Callback activity during one HID request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HidCalls {
    pub save_calls: u32,
    /// The profile the last save callback received.
    pub saved_profile: Option<Profile>,
    pub boot_calls: u32,
}

const NO_CALLS: HidCalls = HidCalls {
    save_calls: 0,
    saved_profile: None,
    boot_calls: 0,
};

// The C callbacks take no user data, so their state is per thread (cargo runs
// tests on several threads).
thread_local! {
    static SAVE_RESULT: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<HidCalls> = const { Cell::new(NO_CALLS) };
}

extern "C" fn on_save_flash(profile: &CProfile) -> bool {
    let mut calls = CALLS.get();
    calls.save_calls += 1;
    calls.saved_profile = Some(from_c(profile));
    CALLS.set(calls);
    SAVE_RESULT.get()
}

extern "C" fn on_enter_bootloader() {
    let mut calls = CALLS.get();
    calls.boot_calls += 1;
    CALLS.set(calls);
}

/// The C engine (`ddpi_engine_t`) behind the method names of
/// [`kf_dirgain::Engine`]; [`COracle::engine`] returns its state as an
/// `Engine` for field-by-field comparison.
#[derive(Clone, Copy)]
pub struct COracle {
    raw: CEngine,
}

impl COracle {
    /// `ddpi_engine_init`.
    pub fn new(base_dpi: u16) -> Self {
        let mut raw = CEngine::default();
        ddpi_engine_init(&mut raw, base_dpi);
        COracle { raw }
    }

    /// A C engine holding exactly `engine`'s state.
    pub fn from_engine(e: &Engine) -> Self {
        COracle {
            raw: CEngine {
                profile: to_c(&e.profile),
                rem_x_q16_16: e.rem_x_q16_16,
                rem_y_q16_16: e.rem_y_q16_16,
                dir_x_q16_16: e.dir_x_q16_16,
                dir_y_q16_16: e.dir_y_q16_16,
                last_octant: e.last_octant,
                last_scale_q16_16: e.last_scale_q16_16,
                last_motion_ms: e.last_motion_ms,
                sensor_dpi: e.sensor_dpi,
                clock_known: e.clock_known,
                clock_ms: e.clock_ms,
            },
        }
    }

    /// The C state, field by field.
    pub fn engine(&self) -> Engine {
        let c = &self.raw;
        Engine {
            profile: from_c(&c.profile),
            rem_x_q16_16: c.rem_x_q16_16,
            rem_y_q16_16: c.rem_y_q16_16,
            dir_x_q16_16: c.dir_x_q16_16,
            dir_y_q16_16: c.dir_y_q16_16,
            last_octant: c.last_octant,
            last_scale_q16_16: c.last_scale_q16_16,
            last_motion_ms: c.last_motion_ms,
            sensor_dpi: c.sensor_dpi,
            clock_known: c.clock_known,
            clock_ms: c.clock_ms,
        }
    }

    pub fn reset_remainders(&mut self) {
        ddpi_engine_reset_remainders(&mut self.raw);
    }

    pub fn notify_idle(&mut self) {
        ddpi_engine_notify_idle(&mut self.raw);
    }

    pub fn set_sensor_dpi(&mut self, sensor_dpi: u16) {
        ddpi_engine_set_sensor_dpi(&mut self.raw, sensor_dpi);
    }

    pub fn set_clock_ms(&mut self, now_ms: u32) {
        ddpi_engine_set_clock_ms(&mut self.raw, now_ms);
    }

    pub fn set_profile(&mut self, profile: &Profile) {
        ddpi_engine_set_profile(&mut self.raw, &to_c(profile));
    }

    pub fn process_delta(&mut self, dx: i16, dy: i16) -> (i16, i16) {
        let (mut out_dx, mut out_dy) = (0, 0);
        ddpi_process_delta(&mut self.raw, dx, dy, &mut out_dx, &mut out_dy);
        (out_dx, out_dy)
    }

    pub fn process_motion(&mut self, dx: i16, dy: i16, now_ms: u32) -> (i16, i16) {
        let (mut out_dx, mut out_dy) = (0, 0);
        ddpi_process_motion(&mut self.raw, dx, dy, now_ms, &mut out_dx, &mut out_dy);
        (out_dx, out_dy)
    }

    /// `ddpi_hid_handle_feature_report`. `save` is `None` for a NULL save
    /// callback, else the value the callback returns; `boot` passes a
    /// bootloader callback. Returns `handled` and the callback activity.
    pub fn handle_feature_report(
        &mut self,
        req: &[u8; HID_PACKET_LEN],
        resp: &mut [u8; HID_PACKET_LEN],
        save: Option<bool>,
        boot: bool,
    ) -> (bool, HidCalls) {
        SAVE_RESULT.set(save.unwrap_or(false));
        CALLS.set(NO_CALLS);
        let save_cb: Option<extern "C" fn(&CProfile) -> bool> = save.map(|_| on_save_flash as _);
        let boot_cb: Option<extern "C" fn()> = boot.then_some(on_enter_bootloader as _);
        let handled = ddpi_hid_handle_feature_report(&mut self.raw, req, resp, save_cb, boot_cb);
        (handled, CALLS.get())
    }

    /// `ddpi_profile_normalize_cpi_headroom` on the engine's profile.
    pub fn normalize_cpi_headroom(&mut self) -> u16 {
        let mut cpi = 0;
        ddpi_profile_normalize_cpi_headroom(&mut self.raw.profile, &mut cpi);
        cpi
    }

    /// `ddpi_profile_set_ccw_frame` on the engine's profile.
    pub fn set_ccw_frame(&mut self, use_ccw: bool) {
        ddpi_profile_set_ccw_frame(&mut self.raw.profile, use_ccw);
    }

    /// `ddpi_profile_set_window_counts` on the engine's profile.
    pub fn set_window_counts(&mut self, window_counts: u8) {
        kf_shim_set_window_counts(&mut self.raw.profile, window_counts);
    }
}

pub fn crc16_ccitt(data: &[u8]) -> u16 {
    // SAFETY: `data` is valid for reads of `data.len()` bytes and the C only
    // reads that range.
    unsafe { ddpi_crc16_ccitt(data.as_ptr(), data.len()) }
}

/// Runs `f` on the C mirror of `profile` and copies the result back.
fn on_profile<R>(profile: &mut Profile, f: impl FnOnce(&mut CProfile) -> R) -> R {
    let mut c = to_c(profile);
    let r = f(&mut c);
    *profile = from_c(&c);
    r
}

pub fn profile_init_default(base_dpi: u16) -> Profile {
    let mut c = CProfile::default();
    ddpi_profile_init_default(&mut c, base_dpi);
    from_c(&c)
}

pub fn sanitize_and_sign(profile: &mut Profile) {
    on_profile(profile, |c| ddpi_profile_sanitize_and_sign(c));
}

pub fn normalize_cpi_headroom(profile: &mut Profile) -> u16 {
    on_profile(profile, |c| {
        let mut cpi = 0;
        ddpi_profile_normalize_cpi_headroom(c, &mut cpi);
        cpi
    })
}

pub fn set_ccw_frame(profile: &mut Profile, use_ccw: bool) {
    on_profile(profile, |c| ddpi_profile_set_ccw_frame(c, use_ccw));
}

pub fn set_window_counts(profile: &mut Profile, window_counts: u8) {
    on_profile(profile, |c| kf_shim_set_window_counts(c, window_counts));
}

pub fn window_counts(profile: &Profile) -> u8 {
    kf_shim_get_window_counts(&to_c(profile))
}

pub fn idle_ms(profile: &Profile) -> u32 {
    kf_shim_get_idle_ms(&to_c(profile))
}

pub fn serialize(profile: &Profile) -> [u8; PROFILE_LEN] {
    let mut out = [0; PROFILE_LEN];
    ddpi_profile_serialize(&to_c(profile), &mut out);
    out
}

pub fn deserialize(buf: &[u8; PROFILE_LEN]) -> Option<Profile> {
    let mut c = CProfile::default();
    ddpi_profile_deserialize(buf, &mut c).then(|| from_c(&c))
}

pub fn is_identity(profile: &Profile) -> bool {
    ddpi_profile_is_identity(&to_c(profile))
}

pub fn compute_scale_q16_16(profile: &Profile, dx: i16, dy: i16) -> (u32, u8) {
    let mut octant = 0;
    let scale = ddpi_compute_scale_q16_16(&to_c(profile), dx, dy, &mut octant);
    (scale, octant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn repr_c_mirrors_match_c_layout() {
        // Same order as LAYOUT in c/kf_shim.c.
        let rust = [
            size_of::<CProfile>(),
            offset_of!(CProfile, magic),
            offset_of!(CProfile, version),
            offset_of!(CProfile, flags),
            offset_of!(CProfile, base_dpi),
            offset_of!(CProfile, global_gain_q8_8),
            offset_of!(CProfile, multipliers_q8_8),
            offset_of!(CProfile, reserved),
            offset_of!(CProfile, crc16),
            size_of::<CEngine>(),
            offset_of!(CEngine, profile),
            offset_of!(CEngine, rem_x_q16_16),
            offset_of!(CEngine, rem_y_q16_16),
            offset_of!(CEngine, dir_x_q16_16),
            offset_of!(CEngine, dir_y_q16_16),
            offset_of!(CEngine, last_octant),
            offset_of!(CEngine, last_scale_q16_16),
            offset_of!(CEngine, last_motion_ms),
            offset_of!(CEngine, sensor_dpi),
            offset_of!(CEngine, clock_known),
            offset_of!(CEngine, clock_ms),
        ];
        for (i, &want) in rust.iter().enumerate() {
            assert_eq!(kf_shim_layout(i), want, "layout entry {i}");
        }
        assert_eq!(
            kf_shim_layout(rust.len()),
            usize::MAX,
            "C lists more entries"
        );
        assert_eq!((rust[0], rust[9]), (PROFILE_LEN, 64));
    }
}
