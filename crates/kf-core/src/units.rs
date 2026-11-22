//! Unit newtypes. Record fields carry their unit in the name (`t_ns`,
//! `yaw_rad`, DATA_SPECIFICATION.md §1); these types carry it through
//! function parameters, so a degree value cannot reach a radian parameter.
//!
//! There is deliberately no `Counts` type: motion values are in the source's
//! declared [`Units`](crate::provenance::Units) × [`ValueScale`], and they are
//! sensor counts only when the source declares `counts`. A counts newtype on
//! every motion field would label processed pointer movement as counts.

use core::f64::consts::PI;

/// Degrees → radians factor: `PI / 180`, rounded once (the factor
/// `f64::to_radians` uses).
pub const RAD_PER_DEG: f64 = PI / 180.0;
/// Radians → degrees factor: `180 / PI`, rounded once.
pub const DEG_PER_RAD: f64 = 180.0 / PI;

/// An angle in radians (the unit of every angle in data and computation).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Radians(pub f64);

/// An angle in degrees (UI values and published game constants).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Degrees(pub f64);

impl Radians {
    /// One multiplication by [`DEG_PER_RAD`].
    pub fn to_degrees(self) -> Degrees {
        Degrees(self.0 * DEG_PER_RAD)
    }
}

impl Degrees {
    /// One multiplication by [`RAD_PER_DEG`].
    pub fn to_radians(self) -> Radians {
        Radians(self.0 * RAD_PER_DEG)
    }
}

/// Nanoseconds on a declared clock: the session clock for receipt times
/// (`t_ns`), the source's own clock domain for source stamps (`src_ts_ns`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nanos(pub i64);

/// Aspect ratio as integer width : height (a pixel size such as 2560×1440 or
/// a nominal ratio such as 16:9). Both are non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Aspect {
    width: u32,
    height: u32,
}

impl Aspect {
    pub const R4_3: Aspect = Aspect {
        width: 4,
        height: 3,
    };
    pub const R16_9: Aspect = Aspect {
        width: 16,
        height: 9,
    };
    pub const R16_10: Aspect = Aspect {
        width: 16,
        height: 10,
    };

    /// `None` if either side is zero.
    pub const fn new(width: u32, height: u32) -> Option<Aspect> {
        if width == 0 || height == 0 {
            None
        } else {
            Some(Aspect { width, height })
        }
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    /// width / height, one rounding.
    pub fn ratio(self) -> f64 {
        f64::from(self.width) / f64::from(self.height)
    }

    /// `self.ratio() / basis.ratio()` computed as (w·h_basis) / (h·w_basis)
    /// in integers, so it is rounded once (exact products below 2⁵³, which
    /// holds for any side ≤ 94 906 265).
    pub fn ratio_to(self, basis: Aspect) -> f64 {
        let num = u64::from(self.width) * u64::from(basis.height);
        let den = u64::from(self.height) * u64::from(basis.width);
        num as f64 / den as f64
    }
}

/// `value_scale` of a source (DATA_SPECIFICATION.md §6.2): motion values are
/// stored as units × scale. 1 for integer units (counts), 256 for Wayland
/// `wl_fixed`, 1024 for rounded fractional browser/macOS values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ValueScale {
    X1 = 1,
    X256 = 256,
    X1024 = 1024,
}

impl ValueScale {
    pub const fn get(self) -> u16 {
        self as u16
    }

    pub const fn from_u16(v: u16) -> Option<ValueScale> {
        match v {
            1 => Some(ValueScale::X1),
            256 => Some(ValueScale::X256),
            1024 => Some(ValueScale::X1024),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_scale_and_aspect() {
        let all = [ValueScale::X1, ValueScale::X256, ValueScale::X1024];
        assert_eq!(all.map(ValueScale::get), [1, 256, 1024]);
        for v in all {
            assert_eq!(ValueScale::from_u16(v.get()), Some(v));
        }
        assert_eq!(ValueScale::from_u16(0), None);
        assert_eq!(ValueScale::from_u16(512), None);
        assert_eq!((Aspect::new(16, 0), Aspect::new(0, 9)), (None, None));
        assert_eq!(Aspect::new(16, 9), Some(Aspect::R16_9));
        assert_eq!(Aspect::R16_9.ratio_to(Aspect::R4_3), 4.0 / 3.0);
    }
}
