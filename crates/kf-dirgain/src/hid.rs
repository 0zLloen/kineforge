//! HID protocol v1 feature-report handler (`.c:666-827`,
//! `spec/hid-protocol-v1.md`). Packet: `'D','P', version, status, seq, 0xDF,
//! opcode, slot, payload[54], CRC-16 LE over bytes 0..62`.

use crate::{
    BOOTLOADER_MAGIC_ARG, DEFAULT_WINDOW_COUNTS, DPI_MAX, Engine, HID_NAMESPACE, HID_PACKET_LEN,
    MAX_WINDOW_COUNTS, PROFILE_LEN, PROFILE_VERSION, Profile, Q8_8_MAX, Q8_8_MIN, cmd, crc16_ccitt,
    flags, read_u16_le, read_u32_le, status, write_u16_le, write_u32_le,
};

impl Engine {
    /// Handles one 64-byte request and fills `resp`. Returns false (and leaves
    /// `resp` untouched) unless the packet carries the `'D','P'` / 0xDF
    /// framing. Pass `None` for a callback the device cannot do; that command
    /// then answers [`status::ERR_UNSUPPORTED`]. SIMULATE_DELTA runs on this
    /// live engine and mutates its state, like the C.
    /// Mirrors `ddpi_hid_handle_feature_report` (`.c:666-827`).
    pub fn handle_feature_report(
        &mut self,
        req: &[u8; HID_PACKET_LEN],
        resp: &mut [u8; HID_PACKET_LEN],
        on_save_flash: Option<&mut dyn FnMut(&Profile) -> bool>,
        on_enter_bootloader: Option<&mut dyn FnMut()>,
    ) -> bool {
        if req[0] != 0x44 || req[1] != 0x50 || req[5] != HID_NAMESPACE {
            return false;
        }

        *resp = [0; HID_PACKET_LEN];
        resp[0] = 0x44; // 'D'
        resp[1] = 0x50; // 'P'
        resp[2] = PROFILE_VERSION;
        resp[4] = req[4]; // sequence number
        resp[5] = HID_NAMESPACE;
        resp[6] = req[6]; // opcode
        resp[7] = req[7]; // slot

        if req[2] != PROFILE_VERSION || crc16_ccitt(&req[..62]) != read_u16_le(req, 62) {
            resp[3] = status::ERR_CRC_OR_FMT;
        } else {
            resp[3] = self.run_command(req, resp, on_save_flash, on_enter_bootloader);
        }

        let crc = crc16_ccitt(&resp[..62]);
        write_u16_le(resp, 62, crc);
        true
    }

    /// The opcode switch of `.c:701-822`: writes the payload (bytes 8..)
    /// and returns the status byte.
    fn run_command(
        &mut self,
        req: &[u8; HID_PACKET_LEN],
        resp: &mut [u8; HID_PACKET_LEN],
        on_save_flash: Option<&mut dyn FnMut(&Profile) -> bool>,
        on_enter_bootloader: Option<&mut dyn FnMut()>,
    ) -> u8 {
        let payload = &mut resp[8..];
        let mut result = status::OK;

        match req[6] {
            cmd::GET_CAPABILITIES => {
                payload[..4].copy_from_slice(b"DP01");
                payload[4] = PROFILE_VERSION;
                payload[5] = 1; // max_slots: one profile, the slot byte is ignored
                payload[6] = 0; // active_slot
                // Only what this engine/adapter can do; the hardware bits 0
                // (wired_1000hz) and 2 (paw3370_sensor) are unknown here.
                let mut caps = 0x10 | 0x20; // travel_window, linear_2x2
                if on_save_flash.is_some() {
                    caps |= 0x02; // nvs_storage
                }
                if on_enter_bootloader.is_some() {
                    caps |= 0x08; // bootloader_reentry
                }
                if self.sensor_dpi != 0 {
                    caps |= 0x40; // sensor DPI known: ONLY_AT_BASE_DPI works
                }
                if self.clock_known != 0 {
                    caps |= 0x80; // clock_ms valid
                }
                payload[7] = caps;
                write_u16_le(payload, 8, 50); // literal 50 in the C (== DPI_MIN)
                write_u16_le(payload, 10, DPI_MAX);
                write_u16_le(payload, 12, Q8_8_MIN);
                write_u16_le(payload, 14, Q8_8_MAX);
                payload[16] = DEFAULT_WINDOW_COUNTS;
                payload[17] = MAX_WINDOW_COUNTS;
                write_u16_le(payload, 18, self.sensor_dpi);
                write_u32_le(payload, 20, self.clock_ms);
            }

            cmd::GET_PROFILE => {
                payload[..PROFILE_LEN].copy_from_slice(&self.profile.serialize());
            }

            cmd::SET_PROFILE => match req[8..].first_chunk().and_then(Profile::deserialize) {
                Some(incoming) => {
                    self.set_profile(&incoming);
                    payload[..PROFILE_LEN].copy_from_slice(&self.profile.serialize());
                }
                None => result = status::ERR_CRC_OR_FMT,
            },

            cmd::SAVE_FLASH => {
                match on_save_flash {
                    None => result = status::ERR_UNSUPPORTED,
                    Some(save) => {
                        if !save(&self.profile) {
                            result = status::ERR_CRC_OR_FMT;
                        }
                    }
                }
                payload[..PROFILE_LEN].copy_from_slice(&self.profile.serialize());
            }

            cmd::RESET_DEFAULTS => {
                // Keep the device's base CPI, the reported sensor DPI and the
                // clock; only the curve returns to neutral.
                *self = Engine {
                    sensor_dpi: self.sensor_dpi,
                    clock_known: self.clock_known,
                    clock_ms: self.clock_ms,
                    ..Engine::new(self.profile.base_dpi)
                };
                payload[..PROFILE_LEN].copy_from_slice(&self.profile.serialize());
            }

            cmd::SET_ENABLED => {
                match req[8] {
                    // Calibration bypass: stays enabled, temporarily 1.00x.
                    2 => {
                        self.profile.flags |= flags::BYPASS_FOR_CALIBRATION;
                        self.reset_remainders();
                    }
                    0 => {
                        self.profile.flags &= !(flags::ENABLED | flags::BYPASS_FOR_CALIBRATION);
                        self.reset_remainders();
                    }
                    _ => {
                        self.profile.flags |= flags::ENABLED;
                        self.profile.flags &= !flags::BYPASS_FOR_CALIBRATION;
                    }
                }
                self.profile.sanitize_and_sign();
                payload[0] = self.profile.flags;
            }

            cmd::SIMULATE_DELTA => {
                let in_dx = read_u16_le(req, 8) as i16;
                let in_dy = read_u16_le(req, 10) as i16;
                let (out_dx, out_dy) = self.process_delta(in_dx, in_dy);
                write_u16_le(payload, 0, out_dx as u16);
                write_u16_le(payload, 2, out_dy as u16);
                write_u32_le(payload, 4, self.rem_x_q16_16 as u32);
                write_u32_le(payload, 8, self.rem_y_q16_16 as u32);
                payload[12] = self.last_octant;
                write_u32_le(payload, 13, self.last_scale_q16_16);
            }

            cmd::ENTER_BOOTLOADER => {
                if read_u32_le(req, 8) != BOOTLOADER_MAGIC_ARG {
                    result = status::ERR_RANGE;
                } else if let Some(enter) = on_enter_bootloader {
                    enter();
                } else {
                    result = status::ERR_UNSUPPORTED;
                }
            }

            _ => result = status::ERR_UNSUPPORTED,
        }

        result
    }
}
