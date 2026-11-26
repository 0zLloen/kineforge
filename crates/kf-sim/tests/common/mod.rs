//! Shared test corpus: synthetic sessions played by a deterministic aiming
//! bot (EXPERIMENT_PLAN E2/E10). Every value derives from fixed seeds with
//! IEEE-exact arithmetic and the pinned `libm`, so the corpus is identical
//! on every target. Input level `synthetic`, evidence `simulated_truth`.
#![allow(dead_code)]

use kf_core::input::{button, button_state};
use kf_core::records::control_action as act;
use kf_core::{Button, InputEvent, LinearSensitivity, Motion, Nanos, ValueScale};
use kf_sim::rng::Xoshiro256pp;
use kf_sim::{Control, FlickConfig, FrameIn, Scenario, Sim, SimConfig, SimRecord};

pub const MS: i64 = 1_000_000;

/// 0.022°/unit (Source), the common reference sensitivity.
pub fn source_sens(sens: f64) -> LinearSensitivity {
    let per = 0.022 * (core::f64::consts::PI / 180.0);
    LinearSensitivity {
        yaw_rad_per_unit: per,
        pitch_rad_per_unit: per,
        sens_x: sens,
        sens_y: sens,
        invert_y: false,
    }
}

/// A corpus variant: configuration and input shape.
#[derive(Clone, Debug)]
pub struct Variant {
    pub name: &'static str,
    pub cfg: SimConfig,
    /// Driving-source reports per millisecond (1 = 1 kHz, 8 = 8 kHz).
    pub per_ms: u32,
    /// Session length.
    pub duration_ms: i64,
    /// Bot RNG seed.
    pub bot_seed: u64,
    /// Occasional violent vertical flicks that hit the pitch clamp.
    pub slam_pitch: bool,
    /// Source stamps on every report (gain clock) or receipt-only.
    pub src_ts: bool,
}

pub fn gain_profile() -> kf_dirgain::Profile {
    let mut p = kf_dirgain::Profile::new_default(800);
    p.flags |= kf_dirgain::flags::ENABLED;
    p.multipliers_q8_8 = [256, 300, 384, 220, 512, 256, 180, 330];
    p.reserved = 16; // travel window 16 counts
    p.sanitize_and_sign();
    p
}

pub fn variants() -> Vec<Variant> {
    let base = SimConfig::new(source_sens(1.6), ValueScale::X1, 0x5eed0001);
    vec![
        Variant {
            name: "counts-1k",
            cfg: base.clone(),
            per_ms: 1,
            duration_ms: 24_000,
            bot_seed: 1,
            slam_pitch: false,
            src_ts: false,
        },
        Variant {
            name: "gain-8k",
            cfg: SimConfig {
                gain: Some(gain_profile()),
                seed: 0x0dd0_0001,
                ..base.clone()
            },
            per_ms: 8,
            duration_ms: 6_000,
            bot_seed: 2,
            slam_pitch: true,
            src_ts: true,
        },
        Variant {
            name: "fractional-multi",
            cfg: SimConfig {
                // Browser-like fractional units: value_scale 1024.
                sensitivity: source_sens(0.8),
                value_scale: ValueScale::X1024,
                seed: 42,
                scenario: Scenario::StaticFlick(FlickConfig {
                    targets: 48,
                    concurrent: 3,
                    directions: 16,
                    lifetime_ms: 900,
                    time_limit_ms: 7_000,
                    ..FlickConfig::default()
                }),
                ..base.clone()
            },
            per_ms: 2,
            duration_ms: 8_000,
            bot_seed: 3,
            slam_pitch: true,
            src_ts: false,
        },
    ]
}

/// A recorded stream: the driving source's events and the session length.
#[derive(Clone, Debug)]
pub struct Stream {
    pub events: Vec<InputEvent>,
    pub end_ns: i64,
}

/// Units the camera must turn to look at `p` (eye at the origin).
fn units_to(p: [f64; 3], cfg: &SimConfig) -> (f64, f64) {
    let s = &cfg.sensitivity;
    let scale = f64::from(cfg.value_scale.get());
    let yaw = libm::atan2(p[0], -p[2]);
    let pitch = libm::atan2(p[1], libm::hypot(p[0], p[2]));
    let yu = yaw * scale / (s.yaw_rad_per_unit * s.sens_x);
    let pu = -pitch * scale / (s.pitch_rad_per_unit * s.sens_y);
    (yu, pu)
}

/// Plays one trial with the bot at 1 ms frames and returns its input.
/// Starts the trial in frame 0 (phase 0) and ends the session at
/// `duration_ms`.
pub fn record_stream(v: &Variant) -> Stream {
    let mut sim = Sim::new(v.cfg.clone()).unwrap();
    let mut bot = Xoshiro256pp::from_seed(v.bot_seed);
    let mut events = Vec::new();
    let mut seq = 0u32;
    let mut out = Vec::new();
    let mut button_down = false;
    let mut slam: i32 = 0;
    // Sub-unit motion carried to the next report, like a real sensor.
    let (mut carry_x, mut carry_y) = (0f64, 0f64);
    // Human-ish pacing: a reaction delay per target (sometimes long enough
    // to let a target time out), then a smooth approach.
    let (mut current, mut react_until) = (None, 0i64);
    for ms in 0..v.duration_ms {
        let cutoff = (ms + 1) * MS;
        let mut frame_events = Vec::new();
        let first = sim.targets().first().map(|t| t.id);
        if first != current {
            current = first;
            react_until = ms + 120 + bot.below(250) as i64;
            if bot.below(6) == 0 {
                react_until += 1100;
            }
        }
        for j in 0..v.per_ms {
            let t = ms * MS + i64::from(j + 1) * (MS / i64::from(v.per_ms));
            let ts = v.src_ts.then_some(Nanos(t - 37_123));
            let cam = sim.camera();
            let (mut dx, mut dy) = (0f64, 0f64);
            if let Some(target) = sim.targets().first()
                && ms >= react_until
            {
                let (yu, pu) = units_to(target.pos_m, &v.cfg);
                dx = (yu - cam.yaw_units() as f64) * 0.02 / f64::from(v.per_ms);
                dy = (pu - cam.pitch_units() as f64) * 0.02 / f64::from(v.per_ms);
            }
            // Tremor and occasional overshoot.
            dx += bot.range_f64(-2.5, 2.5) * f64::from(v.cfg.value_scale.get()) / 8.0;
            dy += bot.range_f64(-2.5, 2.5) * f64::from(v.cfg.value_scale.get()) / 8.0;
            if v.slam_pitch && slam == 0 && bot.below(3000 * u64::from(v.per_ms)) == 0 {
                let n = 40 * v.per_ms as i32;
                slam = if bot.below(2) == 0 { n } else { -n };
            }
            if slam != 0 {
                dy += f64::from(slam.signum()) * 3000.0 / f64::from(v.per_ms)
                    * f64::from(v.cfg.value_scale.get());
                slam -= slam.signum();
            }
            let (wx, wy) = (dx + carry_x, dy + carry_y);
            let (ix, iy) = (wx as i32, wy as i32);
            (carry_x, carry_y) = (wx - f64::from(ix), wy - f64::from(iy));
            let m = Motion {
                t_ns: Nanos(t),
                src_ts_ns: ts,
                dx: ix,
                dy: iy,
                seq,
                source: v.cfg.driving_source,
                flags: 0,
            };
            seq = seq.wrapping_add(1);
            frame_events.push(InputEvent::Motion(m));
        }
        // Click when the aim is within a jittered fraction of the target's
        // angular radius (> 1 sometimes: a near miss), plus the odd random
        // click; release one millisecond later.
        let bot_threshold = bot.range_f64(0.3, 1.2);
        let cam = sim.camera();
        let on = sim.targets().iter().any(|t| {
            let f = cam.forward();
            let a = kf_sim::geom::angle_between(f, t.pos_m);
            a < bot_threshold * libm::asin(t.radius_m / kf_sim::geom::norm(t.pos_m))
        });
        let want = on || bot.below(900) == 0;
        if want != button_down || button_down {
            let down = !button_down && want;
            let b = Button {
                t_ns: Nanos(cutoff),
                src_ts_ns: v.src_ts.then_some(Nanos(cutoff - 37_123)),
                seq,
                source: v.cfg.driving_source,
                button: button::LEFT,
                state: if down { button_state::DOWN } else { 0 },
            };
            seq = seq.wrapping_add(1);
            button_down = down;
            frame_events.push(InputEvent::Button(b));
        }
        // Records the simulation must not apply: an absolute packet (a
        // big jump), a wheel notch, a right click (down then up).
        let t_odd = Nanos(cutoff); // t_ns never goes back along seq
        if bot.below(500) == 0 {
            frame_events.push(InputEvent::Motion(Motion {
                t_ns: t_odd,
                src_ts_ns: None,
                dx: 30_000,
                dy: -20_000,
                seq,
                source: v.cfg.driving_source,
                flags: kf_core::input::event_flags::ABSOLUTE,
            }));
            seq = seq.wrapping_add(1);
        }
        if bot.below(300) == 0 {
            frame_events.push(InputEvent::Wheel(kf_core::Wheel {
                t_ns: t_odd,
                src_ts_ns: None,
                dv: 120,
                dh: 0,
                seq,
                source: v.cfg.driving_source,
                flags: 0,
            }));
            seq = seq.wrapping_add(1);
        }
        if bot.below(400) == 0 {
            for state in [button_state::DOWN, 0] {
                frame_events.push(InputEvent::Button(Button {
                    t_ns: t_odd,
                    src_ts_ns: None,
                    seq,
                    source: v.cfg.driving_source,
                    button: button::RIGHT,
                    state,
                }));
                seq = seq.wrapping_add(1);
            }
        }
        let before = if ms == 0 {
            vec![control(0, act::START_TRIAL)]
        } else {
            vec![]
        };
        let f = FrameIn {
            frame: ms as u64,
            t_start_ns: Nanos(ms * MS),
            t_input_cutoff_ns: Nanos(cutoff),
            ticks: None,
        };
        sim.run_frame(f, &before, frame_events.iter().copied(), &[], &mut out)
            .unwrap();
        out.clear();
        events.extend(frame_events);
    }
    Stream {
        events,
        end_ns: v.duration_ms * MS,
    }
}

pub fn control(t_ns: i64, action: u8) -> Control {
    Control {
        t_ns: Nanos(t_ns),
        action,
        reason: 0,
        new_source: None,
    }
}

fn event_t(e: &InputEvent) -> i64 {
    match e {
        InputEvent::Motion(m) => m.t_ns.0,
        InputEvent::Button(b) => b.t_ns.0,
        InputEvent::Wheel(w) => w.t_ns.0,
        InputEvent::Gap(g) => g.t_ns.0,
    }
}

/// One frame as the live loop ran it: enough to replay it.
#[derive(Clone, Debug)]
pub struct FrameLog {
    pub input: FrameIn,
    pub before: Vec<Control>,
    pub events: Vec<InputEvent>,
    pub after: Vec<Control>,
}

/// A live run: every frame's inputs and the records it produced.
pub struct Run {
    pub frames: Vec<FrameLog>,
    pub records: Vec<SimRecord>,
    pub sim: Sim,
}

/// Runs `stream` live with frame cutoffs `cutoffs` (ascending, the last ≥
/// the stream end). `controls` are (session time, phase, control): each
/// goes into the first frame whose cutoff is ≥ its time. The trial starts
/// in frame 0 (phase 0); the session ends in the last frame (phase 1).
pub fn run_live(
    cfg: &SimConfig,
    stream: &Stream,
    cutoffs: &[i64],
    controls: &[(i64, u8, Control)],
) -> Run {
    let mut sim = Sim::new(cfg.clone()).unwrap();
    let (mut frames, mut records, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let mut ev = stream.events.iter().peekable();
    let mut ctl = controls.iter().peekable();
    let mut prev = 0;
    for (i, &cutoff) in cutoffs.iter().enumerate() {
        let mut before = if i == 0 {
            vec![control(0, act::START_TRIAL)]
        } else {
            vec![]
        };
        let mut after = Vec::new();
        while let Some(&&(t, phase, c)) = ctl.peek() {
            if t > cutoff {
                break;
            }
            if phase == 0 {
                before.push(c)
            } else {
                after.push(c)
            }
            ctl.next();
        }
        if i + 1 == cutoffs.len() {
            after.push(control(cutoff, act::END));
        }
        let mut events = Vec::new();
        while let Some(e) = ev.peek() {
            if event_t(e) > cutoff {
                break;
            }
            events.push(**e);
            ev.next();
        }
        let input = FrameIn {
            frame: i as u64,
            t_start_ns: Nanos(prev),
            t_input_cutoff_ns: Nanos(cutoff),
            ticks: None,
        };
        sim.run_frame(input, &before, events.iter().copied(), &after, &mut out)
            .unwrap();
        // Record the authoritative tick count for replay.
        let ticks = out.iter().find_map(|r| match r {
            SimRecord::Frame(f) => Some(f.ticks),
            _ => None,
        });
        frames.push(FrameLog {
            input: FrameIn { ticks, ..input },
            before,
            events,
            after,
        });
        records.append(&mut out);
        prev = cutoff;
    }
    Run {
        frames,
        records,
        sim,
    }
}

/// Frame cutoffs at a constant rate up to `end_ns` (the last one = end).
pub fn cutoffs_at(fps: u32, end_ns: i64) -> Vec<i64> {
    let mut v = Vec::new();
    let mut i = 1i64;
    loop {
        let c = (i128::from(i) * 1_000_000_000 / i128::from(fps)) as i64;
        if c >= end_ns {
            v.push(end_ns);
            return v;
        }
        v.push(c);
        i += 1;
    }
}

/// Frame cutoffs with random intervals in [lo_us, hi_us] up to `end_ns`.
pub fn cutoffs_random(seed: u64, lo_us: u64, hi_us: u64, end_ns: i64) -> Vec<i64> {
    let mut r = Xoshiro256pp::from_seed(seed);
    let (mut v, mut c) = (Vec::new(), 0i64);
    loop {
        c += ((lo_us + r.below(hi_us - lo_us + 1)) * 1000) as i64;
        if c >= end_ns {
            v.push(end_ns);
            return v;
        }
        v.push(c);
    }
}

/// The simulation records (camera, targets, shots), which E2 requires to be
/// independent of frame boundaries.
pub fn sim_records(r: &[SimRecord]) -> Vec<SimRecord> {
    r.iter()
        .filter(|r| {
            matches!(
                r,
                SimRecord::Camera(_) | SimRecord::Target(_) | SimRecord::Shot(_)
            )
        })
        .cloned()
        .collect()
}
