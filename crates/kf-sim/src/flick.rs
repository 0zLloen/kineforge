//! Static flick, scenario `flick.static` version 1 (AIMLABS_RESEARCH.md §5,
//! row 1 — KineForge's own design). Stationary spheres appear around a fixed
//! home direction (yaw 0, pitch 0); a kill or a timeout frees a slot and
//! the replacement spawns in the work of the resolving tick (for a kill,
//! the tick in progress when the shot applied; for a timeout, the same tick). Target k of trial i is a pure
//! function of (seed, config, i, k), independent of the player.

use crate::geom::V3;
use crate::rng::Xoshiro256pp;
use core::f64::consts::{FRAC_PI_2, TAU};
use kf_core::units::RAD_PER_DEG;

/// Scenario id in the session header.
pub const SCENARIO_ID: &str = "flick.static";
/// Scenario version in the session header.
pub const SCENARIO_VERSION: u32 = 1;
/// Most targets alive at once (bounds per-tick work and the buffers).
pub const MAX_CONCURRENT: u32 = 64;

/// Static-flick parameters. Every field is required in a header and unknown
/// fields are refused: the config is closed, versioned by the scenario
/// version (no defaults are filled in, so a recorded config means what it
/// says).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(deny_unknown_fields)
)]
pub struct FlickConfig {
    /// Targets per trial.
    pub targets: u32,
    /// Targets alive at once.
    pub concurrent: u32,
    /// Eye-to-centre distance.
    pub distance_m: f64,
    /// Angular offset of a target centre from the home direction, uniform
    /// in [min, max]. The flick the player makes is an outcome (from the
    /// previous aim point), not this parameter.
    pub offset_min_rad: f64,
    pub offset_max_rad: f64,
    /// Balanced direction set: directions 2π·j/N around home, each used
    /// once per block of N targets in a shuffled order.
    pub directions: u32,
    /// Angular radius α, uniform in [min, max]; `radius_m = distance × sin α`
    /// (the sphere then subtends exactly α).
    pub radius_min_rad: f64,
    pub radius_max_rad: f64,
    /// Lifetime of a target; 0 = until killed.
    pub lifetime_ms: u32,
    /// Trial time limit; 0 = none (the trial ends when every target is
    /// resolved).
    pub time_limit_ms: u32,
}

impl Default for FlickConfig {
    fn default() -> FlickConfig {
        FlickConfig {
            targets: 32,
            concurrent: 1,
            distance_m: 10.0,
            offset_min_rad: 5.0 * RAD_PER_DEG,
            offset_max_rad: 14.0 * RAD_PER_DEG,
            directions: 8,
            radius_min_rad: 0.75 * RAD_PER_DEG,
            radius_max_rad: 1.5 * RAD_PER_DEG,
            lifetime_ms: 0,
            time_limit_ms: 0,
        }
    }
}

impl FlickConfig {
    /// `Err` names the first invalid field.
    pub fn validate(&self) -> Result<(), &'static str> {
        let angle = |a: f64| a.is_finite() && (0.0..FRAC_PI_2).contains(&a);
        if self.targets == 0 {
            return Err("targets must be ≥ 1");
        }
        if self.concurrent == 0 || self.concurrent > self.targets.min(MAX_CONCURRENT) {
            return Err("concurrent must be in 1..=min(targets, 64)");
        }
        if !(self.distance_m.is_finite() && self.distance_m > 0.0) {
            return Err("distance_m must be finite and > 0");
        }
        if !(angle(self.offset_min_rad)
            && angle(self.offset_max_rad)
            && self.offset_min_rad <= self.offset_max_rad)
        {
            return Err("offsets must satisfy 0 ≤ min ≤ max < π/2");
        }
        if self.directions == 0
            || self.directions > 360
            || !self.targets.is_multiple_of(self.directions)
        {
            return Err("directions must be in 1..=360 and divide targets");
        }
        if !(angle(self.radius_min_rad)
            && angle(self.radius_max_rad)
            && self.radius_min_rad > 0.0
            && self.radius_min_rad <= self.radius_max_rad)
        {
            return Err("radii must satisfy 0 < min ≤ max < π/2");
        }
        Ok(())
    }
}

/// A target as generated: centre (metres, eye at the origin) and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetSpec {
    pub pos: V3,
    pub radius_m: f64,
}

/// The target sequence of one trial.
#[derive(Clone, Debug, PartialEq)]
pub struct FlickGen {
    rng: Xoshiro256pp,
    perm: Vec<u32>,
    k: u32,
}

impl FlickGen {
    /// Trial `trial`'s generator: `Xoshiro256pp::from_seed(seed)` jumped
    /// `trial` times (one non-overlapping 2¹²⁸ subsequence per trial).
    pub fn new(seed: u64, trial: u32, cfg: &FlickConfig) -> FlickGen {
        let mut rng = Xoshiro256pp::from_seed(seed);
        for _ in 0..trial {
            rng.jump();
        }
        FlickGen::from_rng(rng, cfg)
    }

    /// A trial's generator from its (already jumped) RNG state.
    pub(crate) fn from_rng(rng: Xoshiro256pp, cfg: &FlickConfig) -> FlickGen {
        FlickGen {
            rng,
            perm: (0..cfg.directions).collect(),
            k: 0,
        }
    }

    /// Target k (k = 0, 1, …). Draw order: at k ≡ 0 (mod N) shuffle the
    /// direction indices; then offset `a`, then angular radius `α`. Centre
    /// = `[R·(sin a·cos φ), R·(sin a·sin φ), −(R·cos a)]` with φ = 2π·j/N,
    /// i.e. `a` away from −Z towards direction φ (φ = 0 is right, π/2 up).
    pub fn next(&mut self, cfg: &FlickConfig) -> TargetSpec {
        let n = cfg.directions;
        if self.k.is_multiple_of(n) {
            for (i, d) in self.perm.iter_mut().enumerate() {
                *d = i as u32;
            }
            self.rng.shuffle(&mut self.perm);
        }
        let j = self.perm[(self.k % n) as usize];
        self.k += 1;
        let a = self.rng.range_f64(cfg.offset_min_rad, cfg.offset_max_rad);
        let alpha = self.rng.range_f64(cfg.radius_min_rad, cfg.radius_max_rad);
        let phi = TAU * f64::from(j) / f64::from(n);
        let (sa, ca) = (libm::sin(a), libm::cos(a));
        let (sp, cp) = (libm::sin(phi), libm::cos(phi));
        let r = cfg.distance_m;
        TargetSpec {
            pos: [r * (sa * cp), r * (sa * sp), -(r * ca)],
            radius_m: r * libm::sin(alpha),
        }
    }

    /// Targets generated so far.
    pub fn count(&self) -> u32 {
        self.k
    }

    /// Generator state for the state hash: rng words, permutation, k.
    pub(crate) fn hash_into(&self, h: &mut crate::hash::Fnv) {
        for s in self.rng.s {
            h.u64(s);
        }
        h.u64(self.perm.len() as u64);
        for &p in &self.perm {
            h.u32(p);
        }
        h.u32(self.k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{angle_between, norm};

    #[test]
    fn geometry_and_balance() {
        let cfg = FlickConfig::default();
        cfg.validate().unwrap();
        let mut g = FlickGen::new(7, 0, &cfg);
        let home = [0.0, 0.0, -1.0];
        let mut dirs = Vec::new();
        for _ in 0..cfg.targets {
            let t = g.next(&cfg);
            assert!((norm(t.pos) - cfg.distance_m).abs() < 1e-12);
            let off = angle_between(home, t.pos);
            assert!(off >= cfg.offset_min_rad - 1e-12 && off <= cfg.offset_max_rad + 1e-12);
            // The sphere subtends α: asin(radius / distance) within range.
            let alpha = libm::asin(t.radius_m / cfg.distance_m);
            assert!(alpha >= cfg.radius_min_rad - 1e-15 && alpha <= cfg.radius_max_rad + 1e-15);
            let phi = libm::atan2(t.pos[1], t.pos[0]).rem_euclid(TAU);
            dirs.push((phi / (TAU / 8.0)).round() as u32 % 8);
        }
        // Each block of 8 uses every direction exactly once.
        for block in dirs.chunks(8) {
            let mut b = block.to_vec();
            b.sort_unstable();
            assert_eq!(b, (0..8).collect::<Vec<_>>());
        }
    }

    #[test]
    fn trials_are_independent_and_reproducible() {
        let cfg = FlickConfig::default();
        let seq = |trial| {
            let mut g = FlickGen::new(0x5eed0001, trial, &cfg);
            (0..5).map(|_| g.next(&cfg)).collect::<Vec<_>>()
        };
        assert_eq!(seq(3), seq(3));
        assert_ne!(seq(0), seq(1));
        // Pinned first target of trial 0 (any change re-pins every stimulus).
        let t = seq(0)[0];
        assert_eq!(
            (t.pos.map(f64::to_bits), t.radius_m.to_bits()),
            GOLDEN_FIRST,
            "first target of seed 0x5eed0001 trial 0 changed"
        );
    }

    const GOLDEN_FIRST: ([u64; 3], u64) = (
        [0xbfe407a4949634f9, 0x3fe407a4949634fa, 0xc023ebe69a7576c5],
        0x3fc4fe0cc0992a8c,
    );

    #[test]
    fn validation() {
        let ok = FlickConfig::default();
        let bad = [
            FlickConfig { targets: 0, ..ok },
            FlickConfig {
                concurrent: 0,
                ..ok
            },
            FlickConfig {
                concurrent: 33,
                ..ok
            },
            FlickConfig {
                targets: 128,
                concurrent: 65,
                ..ok
            },
            FlickConfig {
                distance_m: f64::NAN,
                ..ok
            },
            FlickConfig {
                offset_max_rad: 2.0,
                ..ok
            },
            FlickConfig {
                offset_min_rad: 0.3,
                offset_max_rad: 0.2,
                ..ok
            },
            FlickConfig {
                directions: 0,
                ..ok
            },
            FlickConfig {
                directions: 24,
                ..ok
            },
            FlickConfig {
                radius_min_rad: 0.0,
                ..ok
            },
            FlickConfig {
                radius_max_rad: f64::INFINITY,
                ..ok
            },
        ];
        for b in bad {
            assert!(b.validate().is_err(), "{b:?}");
        }
        FlickConfig {
            targets: 48,
            directions: 24,
            ..ok
        }
        .validate()
        .unwrap();
    }
}
