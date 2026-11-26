//! EXPERIMENT_PLAN E10 — cross-target equivalence, and live/replay
//! symmetry. Corpus sessions (every variant, plus one driven through pause,
//! source switch, restart, abort, a second trial and next_block) run at
//! fixed frame schedules; the final chained state hash and a digest of
//! every record are pinned. CI runs this natively (x86-64, aarch64 Linux,
//! macOS, Windows) and on wasm32-wasip1, debug and release.

mod common;

use common::*;
use kf_core::records::{control_action as act, frame_flags};
use kf_core::{InputEvent, Nanos};
use kf_sim::hash::Fnv;
use kf_sim::{Control, Sim, SimRecord, TrialState};

fn digest(records: &[SimRecord]) -> u64 {
    let mut h = Fnv::new();
    for r in records {
        h.bytes(format!("{r:?}").as_bytes());
    }
    h.0
}

/// The controls session: counts-1k input, re-labelled to source 1 after a
/// switch made while paused.
/// Config, input, frame cutoffs and (time, phase, control) list.
type Session = (kf_sim::SimConfig, Stream, Vec<i64>, Vec<(i64, u8, Control)>);

fn controls_session() -> Session {
    let v = &variants()[0];
    let mut stream = record_stream(v);
    let cutoffs = cutoffs_random(11, 2_000, 26_000, stream.end_ns);
    let c = |t: i64, a: u8| control(t, a);
    let switch_at = 1_900 * MS;
    let controls = vec![
        (1_500 * MS, 1, c(1_500 * MS, act::PAUSE)),
        (
            switch_at,
            1,
            Control {
                new_source: Some(1),
                ..c(switch_at, act::SWITCH_SOURCE)
            },
        ),
        (2_200 * MS, 0, c(2_200 * MS, act::RESUME)),
        (3_000 * MS, 0, c(3_000 * MS, act::RESTART_TRIAL)),
        (4_500 * MS, 1, c(4_500 * MS, act::ABORT_TRIAL)),
        (4_800 * MS, 0, c(4_800 * MS, act::START_TRIAL)),
        (5_500 * MS, 0, c(5_500 * MS, act::NEXT_BLOCK)),
    ];
    // Events after the switch frame belong to source 1 (own seq numbers).
    let switch_cutoff = *cutoffs.iter().find(|&&x| x >= switch_at).unwrap();
    let mut seq1 = 0u32;
    for e in &mut stream.events {
        let t = match e {
            InputEvent::Motion(m) => m.t_ns.0,
            InputEvent::Button(b) => b.t_ns.0,
            InputEvent::Wheel(w) => w.t_ns.0,
            InputEvent::Gap(g) => g.t_ns.0,
        };
        if t > switch_cutoff {
            match e {
                InputEvent::Motion(m) => (m.source, m.seq) = (1, seq1),
                InputEvent::Button(b) => (b.source, b.seq) = (1, seq1),
                InputEvent::Wheel(w) => (w.source, w.seq) = (1, seq1),
                InputEvent::Gap(_) => {}
            }
            seq1 += 1;
        }
    }
    (v.cfg.clone(), stream, cutoffs, controls)
}

/// (state hash, record digest, record count) per corpus session.
fn corpus() -> Vec<(String, u64, u64, usize)> {
    let mut out = Vec::new();
    for v in variants() {
        let stream = record_stream(&v);
        for (sched, cutoffs) in [
            ("144", cutoffs_at(144, stream.end_ns)),
            (
                "rand",
                cutoffs_random(v.bot_seed, 500, 40_000, stream.end_ns),
            ),
        ] {
            let run = run_live(&v.cfg, &stream, &cutoffs, &[]);
            out.push((
                format!("{}@{sched}", v.name),
                run.sim.state_hash(),
                digest(&run.records),
                run.records.len(),
            ));
        }
    }
    let (cfg, stream, cutoffs, controls) = controls_session();
    let run = run_live(&cfg, &stream, &cutoffs, &controls);
    out.push((
        "controls".into(),
        run.sim.state_hash(),
        digest(&run.records),
        run.records.len(),
    ));
    out
}

#[test]
fn corpus_hashes_are_pinned() {
    let got = corpus();
    let text: Vec<String> = got
        .iter()
        .map(|(n, h, d, c)| format!("{n} {h:#018x} {d:#018x} {c}"))
        .collect();
    assert_eq!(text, PINNED, "corpus changed:\n{}", text.join("\n"));
}

/// Pinned on x86-64 Linux; must hold on every target (E10).
const PINNED: [&str; 7] = [
    "counts-1k@144 0xb7fa8a9839316432 0xbec8e862af57362e 11798",
    "counts-1k@rand 0x49a08ffde5aff5f1 0x41f131e98ddc7234 7246",
    "gain-8k@144 0x5aef613698a9ad1f 0x118bd736a6e51166 4067",
    "gain-8k@rand 0x81c47e31f27dac04 0x8d9989782f9a55c1 2953",
    "fractional-multi@144 0x48bd2ec5bf3d2315 0xff5176bedc21fdd2 10379",
    "fractional-multi@rand 0xe24a3fe048856cfa 0x8e7510345d36faa1 8905",
    "controls 0x37aff28693aa7079 0xd3ef2b8fbb36f60c 8097",
];

#[test]
fn controls_session_behaves() {
    let (cfg, stream, cutoffs, controls) = controls_session();
    let run = run_live(&cfg, &stream, &cutoffs, &controls);
    let a = run.sim.attempts();
    // restart → Restarted (trial 0), its replay of trial 0 aborted, trial 1
    // started and then ended by the session end.
    let summary: Vec<_> = a.iter().map(|x| (x.ordinal, x.trial, x.outcome)).collect();
    use kf_sim::Outcome::*;
    assert_eq!(
        summary,
        [
            (0, 0, Some(Restarted)),
            (1, 0, Some(Aborted)),
            (2, 1, Some(Ended))
        ]
    );
    assert_eq!(run.sim.trial_state(), TrialState::Ended);
    assert_eq!(run.sim.driving_source(), 1);
    // Paused frames step no ticks and say so.
    let paused: Vec<_> = run
        .records
        .iter()
        .filter_map(|r| match r {
            SimRecord::Frame(f) if f.flags & frame_flags::PAUSED != 0 => Some(f.ticks),
            _ => None,
        })
        .collect();
    assert!(!paused.is_empty() && paused.iter().all(|&t| t == 0));
    // Drains name source 0 before the switch and 1 after it.
    let drains: Vec<_> = run
        .records
        .iter()
        .filter_map(|r| match r {
            SimRecord::Drain(d) => Some(d.source),
            _ => None,
        })
        .collect();
    assert_eq!(drains.first(), Some(&0));
    assert_eq!(drains.last(), Some(&1));
}

/// Replay with the recorded tick counts reproduces every record (frame
/// flags aside: catch-up capping is a live-clock fact) and the state hash,
/// including after a stall that capped catch-up.
#[test]
fn replay_reproduces_live() {
    let mut sessions = Vec::new();
    for v in variants() {
        let stream = record_stream(&v);
        let mut cutoffs = cutoffs_random(v.bot_seed + 100, 500, 30_000, stream.end_ns);
        // A 600 ms stall: more than max_catch_up_ticks (250).
        let at = cutoffs.len() / 2;
        let stall = cutoffs[at - 1] + 600 * MS;
        let before_stall = cutoffs[at - 1];
        cutoffs.retain(|&c| c <= before_stall || c >= stall);
        cutoffs.insert(at, stall);
        sessions.push((v.cfg.clone(), stream, cutoffs, Vec::new(), true));
    }
    let (cfg, stream, cutoffs, controls) = controls_session();
    sessions.push((cfg, stream, cutoffs, controls, false));
    for (cfg, stream, cutoffs, controls, stalled) in sessions {
        let live = run_live(&cfg, &stream, &cutoffs, &controls);
        let mut sim = Sim::new(cfg.clone()).unwrap();
        let mut records = Vec::new();
        for f in &live.frames {
            sim.run_frame(
                f.input,
                &f.before,
                f.events.iter().copied(),
                &f.after,
                &mut records,
            )
            .unwrap();
        }
        let strip = |r: &[SimRecord]| -> Vec<SimRecord> {
            r.iter()
                .cloned()
                .map(|r| match r {
                    SimRecord::Frame(mut f) => {
                        f.flags &= !frame_flags::CATCH_UP_CAPPED;
                        SimRecord::Frame(f)
                    }
                    other => other,
                })
                .collect()
        };
        assert_eq!(strip(&records), strip(&live.records));
        assert_eq!(sim.state_hash(), live.sim.state_hash());
        let capped = live.records.iter().any(
            |r| matches!(r, SimRecord::Frame(f) if f.flags & frame_flags::CATCH_UP_CAPPED != 0),
        );
        assert_eq!(capped, stalled, "the stall (and only it) caps catch-up");
    }
}

/// A recorded frame that steps ticks while paused is refused before any
/// state changes.
#[test]
fn ticks_while_paused_are_refused() {
    let v = &variants()[0];
    let mut sim = Sim::new(v.cfg.clone()).unwrap();
    let mut out = Vec::new();
    let f = |frame, ticks| kf_sim::FrameIn {
        frame,
        t_start_ns: Nanos(0),
        t_input_cutoff_ns: Nanos(10 * MS),
        ticks: Some(ticks),
    };
    let pause = [control(0, act::PAUSE)];
    let before = sim.state_hash();
    assert_eq!(
        sim.run_frame(f(0, 5), &pause, [], &[], &mut out),
        Err(kf_sim::FrameError::TicksWhilePaused { ticks: 5 })
    );
    assert_eq!((sim.state_hash(), out.len()), (before, 0));
    sim.run_frame(f(0, 0), &pause, [], &[], &mut out).unwrap();
    assert_eq!(
        sim.run_frame(f(2, 0), &[], [], &[], &mut out),
        Err(kf_sim::FrameError::FrameNumber {
            expected: 1,
            got: 2
        })
    );
}
