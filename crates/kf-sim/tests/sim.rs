//! Targeted simulation cases: attempt timing, what the state hash covers,
//! gain reset on a source switch, refused frames, extreme timestamps and
//! the per-trial generators.

mod common;

use common::*;
use kf_core::input::{button, button_state};
use kf_core::records::{control_action as act, target_event};
use kf_core::{Button, InputEvent, Motion, Nanos, ValueScale};
use kf_sim::flick::FlickGen;
use kf_sim::{
    ConfigError, Control, FlickConfig, FrameError, FrameIn, Outcome, Scenario, Sim, SimConfig,
    SimRecord,
};

/// One huge sphere straight ahead: any aim within ±80° hits it.
fn ahead(targets: u32) -> SimConfig {
    let mut c = SimConfig::new(source_sens(1.0), ValueScale::X1, 7);
    c.scenario = Scenario::StaticFlick(FlickConfig {
        targets,
        directions: 1,
        offset_min_rad: 0.0,
        offset_max_rad: 0.0,
        radius_min_rad: 1.4,
        radius_max_rad: 1.4,
        ..FlickConfig::default()
    });
    c
}

fn frame(frame: u64, cutoff_ns: i64) -> FrameIn {
    FrameIn {
        frame,
        t_start_ns: Nanos(0),
        t_input_cutoff_ns: Nanos(cutoff_ns),
        ticks: None,
    }
}

fn click(t_ns: i64, seq: u32) -> InputEvent {
    InputEvent::Button(Button {
        t_ns: Nanos(t_ns),
        src_ts_ns: None,
        seq,
        source: 0,
        button: button::LEFT,
        state: button_state::DOWN,
    })
}

fn motion(t_ns: i64, seq: u32, source: u16, dx: i32, dy: i32) -> InputEvent {
    InputEvent::Motion(Motion {
        t_ns: Nanos(t_ns),
        src_ts_ns: None,
        dx,
        dy,
        seq,
        source,
        flags: 0,
    })
}

/// An attempt ended by a control lasts until the simulation's time then,
/// sub-tick remainder included: never shorter than a kill inside it.
#[test]
fn control_end_time_includes_the_remainder() {
    let mut sim = Sim::new(ahead(2)).unwrap();
    let mut out = Vec::new();
    sim.run_frame(
        frame(0, MS),
        &[control(0, act::START_TRIAL)],
        [],
        &[],
        &mut out,
    )
    .unwrap();
    // Cutoff 10.9 ms: 9 more ticks and a 0.9 ms remainder; kill at 10.5 ms.
    sim.run_frame(
        frame(1, 10_900_000),
        &[],
        [click(10_500_000, 0)],
        &[control(10_900_000, act::END)],
        &mut out,
    )
    .unwrap();
    let a = sim.attempts()[0];
    assert_eq!((a.outcome, a.kills), (Some(Outcome::Ended), 1));
    assert_eq!(a.ttk_sum_ns, 9_500_000); // 10.5 ms − spawn instant 1 ms
    assert_eq!(a.duration_ns, 9_900_000); // 10.9 ms − 1 ms
}

/// Finished attempts are part of the state hash: two runs that differ
/// only in when the trial-completing kill happened hash differently.
#[test]
fn finished_attempts_are_hashed() {
    let run = |t_click| {
        let mut sim = Sim::new(ahead(1)).unwrap();
        let mut out = Vec::new();
        sim.run_frame(
            frame(0, MS),
            &[control(0, act::START_TRIAL)],
            [],
            &[],
            &mut out,
        )
        .unwrap();
        sim.run_frame(frame(1, 3 * MS), &[], [click(t_click, 0)], &[], &mut out)
            .unwrap();
        (sim.state_hash(), sim.attempts()[0])
    };
    let (ha, a) = run(1_200_000);
    let (hb, b) = run(1_700_000);
    assert_eq!(
        (a.outcome, b.outcome),
        (Some(Outcome::Complete), Some(Outcome::Complete))
    );
    assert_ne!(a.ttk_sum_ns, b.ttk_sum_ns);
    assert_ne!(ha, hb);
}

/// switch_source starts the new device with fresh gain state.
#[test]
fn switch_source_resets_gain_state() {
    let mut cfg = ahead(1);
    cfg.gain = Some(gain_profile());
    let mut sim = Sim::new(cfg).unwrap();
    let mut out = Vec::new();
    let moves: Vec<_> = (0..20)
        .map(|i| motion(i64::from(i + 1) * 40_000, i, 0, 7, 3))
        .collect();
    sim.run_frame(
        frame(0, MS),
        &[],
        moves,
        &[control(MS, act::PAUSE)],
        &mut out,
    )
    .unwrap();
    let e = sim.camera().gain().unwrap();
    assert!(e.rem_x_q16_16 != 0 || e.dir_x_q16_16 != 0, "{e:?}");
    let switch = Control {
        new_source: Some(1),
        ..control(2 * MS, act::SWITCH_SOURCE)
    };
    sim.run_frame(frame(1, 2 * MS), &[switch], [], &[], &mut out)
        .unwrap();
    let e = sim.camera().gain().unwrap();
    assert_eq!(
        (
            e.rem_x_q16_16,
            e.rem_y_q16_16,
            e.dir_x_q16_16,
            e.dir_y_q16_16
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(sim.driving_source(), 1);
}

/// A recorded frame may step at most one second of ticks; the live clock
/// cannot be configured to step more.
#[test]
fn ticks_are_bounded() {
    let mut sim = Sim::new(ahead(1)).unwrap();
    let mut out = Vec::new();
    let f = FrameIn {
        ticks: Some(1001),
        ..frame(0, 2_000 * MS)
    };
    assert_eq!(
        sim.run_frame(f, &[], [], &[], &mut out),
        Err(FrameError::TooManyTicks { ticks: 1001 })
    );
    // Live: a catch-up limit above sim_hz is clamped to one second.
    let mut c = ahead(1);
    c.max_catch_up_ticks = 5000;
    let mut sim = Sim::new(c).unwrap();
    out.clear();
    sim.run_frame(frame(0, 5_000 * MS), &[], [], &[], &mut out)
        .unwrap();
    assert!(matches!(out.last(), Some(SimRecord::Frame(f)) if f.ticks == 1000));
    // sim_hz itself is bounded (a header could claim 1e9).
    for hz in [20_000, 1_000_000_000] {
        let c = SimConfig {
            sim_hz: hz,
            ..ahead(1)
        };
        assert_eq!(Sim::new(c).err(), Some(ConfigError::SimHz(hz)));
    }
}

/// Extreme (hostile) timestamps neither panic in debug builds nor diverge
/// from release: every difference of timestamps is taken in i128.
#[test]
fn extreme_timestamps_do_not_overflow() {
    let mut sim = Sim::new(ahead(1)).unwrap();
    let mut out = Vec::new();
    let replay = |frame, cutoff, ticks| FrameIn {
        frame,
        t_start_ns: Nanos(0),
        t_input_cutoff_ns: Nanos(cutoff),
        ticks: Some(ticks),
    };
    sim.run_frame(
        replay(0, i64::MIN, 0),
        &[control(0, act::START_TRIAL)],
        [],
        &[],
        &mut out,
    )
    .unwrap();
    sim.run_frame(
        replay(1, i64::MAX, 3),
        &[],
        [motion(0, 0, 0, 5, 5), click(i64::MAX, 1)],
        &[],
        &mut out,
    )
    .unwrap();
    let pause = control(i64::MIN, act::PAUSE);
    let resume = control(i64::MAX, act::RESUME);
    sim.run_frame(replay(2, 0, 0), &[pause], [], &[resume], &mut out)
        .unwrap();
    sim.run_frame(
        replay(3, i64::MIN, 0),
        &[],
        [motion(i64::MIN, 2, 0, 1, 1)],
        &[control(i64::MIN, act::END)],
        &mut out,
    )
    .unwrap();
    assert!(
        sim.attempts()
            .iter()
            .all(|a| a.duration_ns >= 0 && a.ttk_sum_ns >= 0)
    );
}

/// Many start_trial controls cost one RNG jump each (not one per trial
/// index), and trial i still spawns exactly FlickGen::new(seed, i)'s
/// targets; a restart reuses them.
#[test]
fn trial_generators_match_the_reference() {
    let cfg = ahead(1);
    let Scenario::StaticFlick(flick) = cfg.scenario;
    let mut sim = Sim::new(SimConfig {
        scenario: Scenario::StaticFlick(FlickConfig {
            offset_max_rad: 0.3,
            directions: 1,
            ..flick
        }),
        ..cfg.clone()
    })
    .unwrap();
    let Scenario::StaticFlick(flick) = sim.config().scenario.clone();
    let mut out = Vec::new();
    let starts = vec![control(0, act::START_TRIAL); 2000];
    sim.run_frame(frame(0, MS), &starts, [], &[], &mut out)
        .unwrap();
    assert_eq!(sim.attempts().len(), 1999);
    let spawned = |out: &[SimRecord]| {
        out.iter()
            .rev()
            .find_map(|r| match r {
                SimRecord::Target(t) if t.event == target_event::SPAWN => Some(t.pos_m),
                _ => None,
            })
            .unwrap()
    };
    let expect = FlickGen::new(cfg.seed, 1999, &flick).next(&flick).pos;
    assert_eq!(spawned(&out), expect);
    out.clear();
    sim.run_frame(
        frame(1, 2 * MS),
        &[control(MS, act::RESTART_TRIAL)],
        [],
        &[],
        &mut out,
    )
    .unwrap();
    assert_eq!(spawned(&out), expect);
    out.clear();
    sim.run_frame(
        frame(2, 3 * MS),
        &[control(2 * MS, act::START_TRIAL)],
        [],
        &[],
        &mut out,
    )
    .unwrap();
    assert_eq!(
        spawned(&out),
        FlickGen::new(cfg.seed, 2000, &flick).next(&flick).pos
    );
}
