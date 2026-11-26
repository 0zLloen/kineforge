//! The replay driver on logs built from live runs: groups cut at frame
//! boundaries (and before every frame that switches source, as the
//! recorder does), events of each frame in its group, kinds in ascending
//! order, plus a record-only second source.

mod common;

use common::*;
use kf_core::records::control_action as act;
use kf_core::{Annotation, GapReason, InputEvent, Motion, Nanos};
use kf_sim::replay::{Item, Replay, Report};
use kf_sim::{Sim, SimRecord};

/// Group layout knobs.
#[derive(Clone, Copy, Default)]
struct Layout {
    /// Frames per group (cut also before switch frames).
    per_group: usize,
    /// Move this frame's events into the next group (a lag the recorder
    /// never produces).
    lag_frame: Option<usize>,
    /// Do not cut before switch frames.
    no_switch_cut: bool,
}

fn switch_frame(f: &FrameLog) -> bool {
    f.before
        .iter()
        .chain(&f.after)
        .any(|c| c.action == act::SWITCH_SOURCE)
}

/// Builds the item stream of a log from a live run. `extra` holds
/// annotations to add to the group of the given frame.
fn log_of(run: &Run, layout: Layout, extra: &[(usize, Annotation)]) -> Vec<Vec<Item>> {
    // Records of each frame: everything run_frame produced up to its Frame
    // record.
    let mut per_frame: Vec<Vec<SimRecord>> = vec![Vec::new(); run.frames.len()];
    // A frame's records end with its Frame record, then its Drain (if any).
    let (mut i, mut closed) = (0, false);
    for r in &run.records {
        if closed && !matches!(r, SimRecord::Drain(_)) {
            (i, closed) = (i + 1, false);
        }
        per_frame[i].push(r.clone());
        closed |= matches!(r, SimRecord::Frame(_));
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, f) in run.frames.iter().enumerate() {
        let cut = groups.last().is_none_or(|g| g.len() >= layout.per_group)
            || (switch_frame(f) && !layout.no_switch_cut);
        if cut {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(i);
    }
    let mut out = Vec::new();
    let mut carried: Vec<InputEvent> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut events = core::mem::take(&mut carried);
        for &i in g {
            if layout.lag_frame == Some(i) {
                carried.extend(run.frames[i].events.iter().copied());
            } else {
                events.extend(run.frames[i].events.iter().copied());
            }
        }
        // A record-only source (id 9) moving all the time.
        for (k, &i) in g.iter().enumerate() {
            let t = run.frames[i].input.t_input_cutoff_ns;
            events.push(InputEvent::Motion(Motion {
                t_ns: t,
                src_ts_ns: None,
                dx: 1,
                dy: -1,
                seq: (gi * 1000 + k) as u32,
                source: 9,
                flags: 0,
            }));
        }
        let mut items = Vec::new();
        // Ascending kinds: motion, button, wheel, frame, drain, sim_*, annotation.
        for want in 0..3 {
            for e in &events {
                let k = match e {
                    InputEvent::Motion(_) => 0,
                    InputEvent::Button(_) => 1,
                    _ => 2,
                };
                if k == want {
                    items.push(Item::Event(*e));
                }
            }
        }
        let recs: Vec<&SimRecord> = g.iter().flat_map(|&i| &per_frame[i]).collect();
        let pick = |f: &dyn Fn(&SimRecord) -> Option<Item>| {
            recs.iter().filter_map(|r| f(r)).collect::<Vec<_>>()
        };
        items.extend(pick(&|r| match r {
            SimRecord::Frame(f) => Some(Item::Frame(*f)),
            _ => None,
        }));
        items.extend(pick(&|r| match r {
            SimRecord::Drain(d) => Some(Item::Drain(*d)),
            _ => None,
        }));
        for kind in 0..3 {
            items.extend(pick(&|r| match (kind, r) {
                (0, SimRecord::Camera(_)) | (1, SimRecord::Target(_)) | (2, SimRecord::Shot(_)) => {
                    Some(Item::Logged(r.clone()))
                }
                _ => None,
            }));
        }
        items.extend(pick(&|r| match r {
            SimRecord::Control(c) => Some(Item::Control(*c)),
            _ => None,
        }));
        for (fi, a) in extra {
            if g.contains(fi) {
                items.push(Item::Annotation(a.clone()));
            }
        }
        out.push(items);
    }
    out
}

fn replay(cfg: &kf_sim::SimConfig, groups: Vec<Vec<Item>>) -> Report {
    let mut r = Replay::new(cfg.clone()).unwrap();
    for g in groups {
        for item in g {
            r.push(item);
        }
        r.end_group();
    }
    r.finish()
}

fn layout(per_group: usize) -> Layout {
    Layout {
        per_group,
        ..Layout::default()
    }
}

#[test]
fn replays_live_runs_identically() {
    for v in variants() {
        let stream = record_stream(&v);
        let run = run_live(
            &v.cfg,
            &stream,
            &cutoffs_random(v.bot_seed, 400, 30_000, stream.end_ns),
            &[],
        );
        for per_group in [1, 7, 40] {
            let rep = replay(&v.cfg, log_of(&run, layout(per_group), &[]));
            assert!(
                rep.identical(),
                "{} /{per_group}: {:?} {:?}",
                v.name,
                rep.error,
                rep.mismatch
            );
            assert_eq!(rep.state_hash, run.sim.state_hash());
            assert_eq!(rep.frames, run.frames.len() as u64);
            assert_eq!(rep.attempts, run.sim.attempts());
            assert!(rep.compared.iter().all(|&n| n > 0), "{:?}", rep.compared);
        }
    }
}

#[test]
fn replays_source_switches() {
    let v = &variants()[0];
    let stream = record_stream(v);
    let cutoffs = cutoffs_random(5, 2_000, 20_000, stream.end_ns);
    let switch_at = 1_900 * MS;
    let switch_cutoff = *cutoffs.iter().find(|&&c| c >= switch_at).unwrap();
    let mut stream = stream;
    let mut seq1 = 0;
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
    let ctl = |t, a| control(t, a);
    let controls = vec![
        (1_500 * MS, 1, ctl(1_500 * MS, act::PAUSE)),
        (
            switch_at,
            1,
            kf_sim::Control {
                new_source: Some(1),
                ..ctl(switch_at, act::SWITCH_SOURCE)
            },
        ),
        (2_200 * MS, 0, ctl(2_200 * MS, act::RESUME)),
    ];
    let run = run_live(&v.cfg, &stream, &cutoffs, &controls);
    assert_eq!(run.sim.driving_source(), 1);
    let rep = replay(&v.cfg, log_of(&run, layout(9), &[]));
    assert!(rep.identical(), "{:?} {:?}", rep.error, rep.mismatch);
    assert_eq!(rep.state_hash, run.sim.state_hash());
    // Without the cut before the switch frame the start point is ambiguous.
    let rep = replay(
        &v.cfg,
        log_of(
            &run,
            Layout {
                per_group: 1000,
                no_switch_cut: true,
                ..Layout::default()
            },
            &[],
        ),
    );
    assert!(
        rep.error
            .as_deref()
            .is_some_and(|e| e.contains("does not start its group")),
        "{:?}",
        rep.error
    );
}

#[test]
fn reports_the_first_mismatch() {
    let v = &variants()[0];
    let stream = record_stream(v);
    let run = run_live(&v.cfg, &stream, &cutoffs_at(144, stream.end_ns), &[]);
    let mut groups = log_of(&run, layout(10), &[]);
    // Flip the hit flag of the third logged shot.
    let mut n = 0;
    'outer: for g in &mut groups {
        for item in g.iter_mut() {
            if let Item::Logged(SimRecord::Shot(s)) = item {
                n += 1;
                if n == 3 {
                    s.ang_err_rad = f64::from_bits(s.ang_err_rad.to_bits() ^ 1);
                    break 'outer;
                }
            }
        }
    }
    let rep = replay(&v.cfg, groups);
    let m = rep.mismatch.expect("mismatch");
    assert_eq!((m.kind, m.index), ("sim_shot", 2));
    assert!(m.logged.is_some() && m.replayed.is_some());
}

#[test]
fn events_after_their_drain_are_not_replayable() {
    let v = &variants()[0];
    let stream = record_stream(v);
    let run = run_live(&v.cfg, &stream, &cutoffs_at(60, stream.end_ns), &[]);
    let lag = Layout {
        per_group: 5,
        lag_frame: Some(12),
        ..Layout::default()
    };
    let rep = replay(&v.cfg, log_of(&run, lag, &[]));
    assert!(
        rep.error
            .as_deref()
            .is_some_and(|e| e.contains("not in the log at its drain")),
        "{:?}",
        rep.error
    );
}

/// Events dropped from the simulation ring (sim_queue_overflow) are in
/// the log but were never applied; the annotation makes replay skip them.
#[test]
fn skips_sim_queue_overflow() {
    let v = &variants()[0];
    let stream = record_stream(v);
    let run = run_live(&v.cfg, &stream, &cutoffs_at(100, stream.end_ns), &[]);
    // Re-run live with seqs 4000..4100 withheld from the simulation.
    let (from, lost) = (4000u32, 100u32);
    let mut sim = Sim::new(v.cfg.clone()).unwrap();
    let mut frames = run.frames.clone();
    let mut records = Vec::new();
    let mut gap_frame = 0;
    for (i, f) in frames.iter_mut().enumerate() {
        let n = f.events.len();
        f.events.retain(|e| match e {
            InputEvent::Motion(m) => !(from..from + lost).contains(&m.seq),
            InputEvent::Button(b) => !(from..from + lost).contains(&b.seq),
            _ => true,
        });
        if f.events.len() != n && gap_frame == 0 {
            gap_frame = i;
        }
        let input = kf_sim::FrameIn {
            ticks: None,
            ..f.input
        };
        sim.run_frame(
            input,
            &f.before,
            f.events.iter().copied(),
            &f.after,
            &mut records,
        )
        .unwrap();
    }
    let dropped = Run {
        frames: frames.clone(),
        records,
        sim,
    };
    // The log still has every event (from the original run).
    let mut log_run = Run {
        frames: run.frames.clone(),
        records: dropped.records.clone(),
        sim: Sim::new(v.cfg.clone()).unwrap(),
    };
    log_run
        .frames
        .iter_mut()
        .zip(&frames)
        .for_each(|(a, _)| a.input.ticks = None);
    let gap = Annotation::from(kf_core::Gap {
        t_ns: Nanos(0),
        source: Some(0),
        first_seq: from,
        lost: Some(lost),
        reason: GapReason::SimQueueOverflow,
    });
    let rep = replay(
        &v.cfg,
        log_of(&log_run, layout(8), &[(gap_frame, gap.clone())]),
    );
    assert!(rep.identical(), "{:?} {:?}", rep.error, rep.mismatch);
    assert_eq!(rep.state_hash, dropped.sim.state_hash());
    // Without the annotation the replay applies them and diverges.
    let rep = replay(&v.cfg, log_of(&log_run, layout(8), &[]));
    assert!(!rep.identical());
    // An unknown count cannot be replayed.
    let unknown = Annotation { lost: None, ..gap };
    let rep = replay(&v.cfg, log_of(&log_run, layout(8), &[(gap_frame, unknown)]));
    assert!(rep.error.is_some());
}

#[test]
fn seq_wraps_past_u32() {
    let v = &variants()[1]; // 8 kHz: many events
    let mut stream = record_stream(v);
    let start = u32::MAX - 5_000;
    for e in &mut stream.events {
        match e {
            InputEvent::Motion(m) => m.seq = m.seq.wrapping_add(start),
            InputEvent::Button(b) => b.seq = b.seq.wrapping_add(start),
            InputEvent::Wheel(w) => w.seq = w.seq.wrapping_add(start),
            InputEvent::Gap(_) => {}
        }
    }
    let run = run_live(&v.cfg, &stream, &cutoffs_at(240, stream.end_ns), &[]);
    let rep = replay(&v.cfg, log_of(&run, layout(6), &[]));
    assert!(rep.identical(), "{:?} {:?}", rep.error, rep.mismatch);
    assert_eq!(rep.state_hash, run.sim.state_hash());
}

#[test]
fn record_queue_overflow_is_not_replayable() {
    let v = &variants()[0];
    let stream = record_stream(v);
    let run = run_live(&v.cfg, &stream, &cutoffs_at(60, stream.end_ns), &[]);
    let lost = Annotation::from(kf_core::Gap {
        t_ns: Nanos(0),
        source: None,
        first_seq: 0,
        lost: Some(1),
        reason: GapReason::RecordQueueOverflow,
    });
    let rep = replay(&v.cfg, log_of(&run, layout(8), &[(30, lost)]));
    assert!(
        rep.error
            .as_deref()
            .is_some_and(|e| e.contains("record_queue_overflow"))
    );
}

/// The counts-1k input with a pause, a frame that switches source while
/// paused (`before`/`after` lists), and a resume; the switch frame takes no
/// input. Events after it are relabelled to `after_source`.
fn switch_session(before: &[u16], after: &[u16], after_source: u16) -> (kf_sim::SimConfig, Run) {
    let v = &variants()[0];
    let mut stream = record_stream(v);
    let cutoffs = cutoffs_random(5, 2_000, 20_000, stream.end_ns);
    let f = cutoffs.iter().position(|&c| c >= 1_900 * MS).unwrap();
    let (lo, hi) = (cutoffs[f - 1], cutoffs[f]);
    let t_of = |e: &InputEvent| match e {
        InputEvent::Motion(m) => m.t_ns.0,
        InputEvent::Button(b) => b.t_ns.0,
        InputEvent::Wheel(w) => w.t_ns.0,
        InputEvent::Gap(g) => g.t_ns.0,
    };
    stream.events.retain(|e| !(lo < t_of(e) && t_of(e) <= hi));
    if after_source != 0 {
        let mut seq = 0;
        for e in &mut stream.events {
            if t_of(e) > hi {
                match e {
                    InputEvent::Motion(m) => (m.source, m.seq) = (after_source, seq),
                    InputEvent::Button(b) => (b.source, b.seq) = (after_source, seq),
                    InputEvent::Wheel(w) => (w.source, w.seq) = (after_source, seq),
                    InputEvent::Gap(_) => {}
                }
                seq += 1;
            }
        }
    }
    let ctl = |t, a| control(t, a);
    let sw = |s| kf_sim::Control {
        new_source: Some(s),
        ..ctl(hi, act::SWITCH_SOURCE)
    };
    let mut controls = vec![(1_500 * MS, 1, ctl(1_500 * MS, act::PAUSE))];
    controls.extend(before.iter().map(|&s| (hi, 0, sw(s))));
    controls.extend(after.iter().map(|&s| (hi, 1, sw(s))));
    controls.push((2_200 * MS, 0, ctl(2_200 * MS, act::RESUME)));
    let run = run_live(&v.cfg, &stream, &cutoffs, &controls);
    (v.cfg.clone(), run)
}

#[test]
fn switch_away_and_back_in_one_frame() {
    let cases: [(&[u16], &[u16], u16, &str); 5] = [
        (&[1], &[0], 0, "away and back across phases"),
        (&[1, 0], &[], 0, "away and back in phase 0"),
        (&[1], &[1], 1, "twice to the same source"),
        (&[], &[1], 1, "phase-1 switch"),
        (&[0], &[], 0, "switch to the driving source"),
    ];
    for (before, after, src, name) in cases {
        let (cfg, run) = switch_session(before, after, src);
        assert_eq!(run.sim.driving_source(), src, "{name}");
        for per_group in [3, 50] {
            let rep = replay(&cfg, log_of(&run, layout(per_group), &[]));
            assert!(
                rep.identical(),
                "{name}/{per_group}: {:?} {:?}",
                rep.error,
                rep.mismatch
            );
            assert_eq!(rep.state_hash, run.sim.state_hash(), "{name}");
        }
    }
}

/// A source switched to in a group gets that group's sim_queue_overflow
/// ranges too.
#[test]
fn skips_overflow_of_a_source_switched_to() {
    let (cfg, run) = switch_session(&[], &[1], 1);
    // Re-run live with source-1 seqs 400..420 (after the resume, so they
    // matter) withheld from the simulation.
    let (from, lost) = (400u32, 20u32);
    let mut sim = Sim::new(cfg.clone()).unwrap();
    let mut records = Vec::new();
    for f in &run.frames {
        let events: Vec<_> = f
            .events
            .iter()
            .copied()
            .filter(|e| !matches!(e, InputEvent::Motion(m) if m.source == 1 && (from..from + lost).contains(&m.seq)))
            .filter(|e| !matches!(e, InputEvent::Button(b) if b.source == 1 && (from..from + lost).contains(&b.seq)))
            .collect();
        sim.run_frame(f.input, &f.before, events, &f.after, &mut records)
            .unwrap();
    }
    let switch_frame = run
        .frames
        .iter()
        .position(|f| f.after.iter().any(|c| c.action == act::SWITCH_SOURCE))
        .unwrap();
    let log_run = Run {
        frames: run.frames.clone(),
        records,
        sim: Sim::new(cfg.clone()).unwrap(),
    };
    let gap = Annotation::from(kf_core::Gap {
        t_ns: Nanos(0),
        source: Some(1),
        first_seq: from,
        lost: Some(lost),
        reason: GapReason::SimQueueOverflow,
    });
    // One group from the switch frame to the end: the annotation and the
    // drains that skip its seqs share it.
    let rep = replay(
        &cfg,
        log_of(&log_run, layout(100_000), &[(switch_frame, gap)]),
    );
    assert!(rep.identical(), "{:?} {:?}", rep.error, rep.mismatch);
    assert_eq!(rep.state_hash, sim.state_hash());
    // Not vacuous: without the annotation the replay applies them.
    let rep = replay(&cfg, log_of(&log_run, layout(100_000), &[]));
    assert!(rep.mismatch.is_some(), "{:?}", rep.error);
}

#[test]
fn seq_wraps_inside_the_first_group() {
    let v = &variants()[0];
    for start in [u32::MAX - 3, u32::MAX] {
        let mut stream = record_stream(v);
        for e in &mut stream.events {
            match e {
                InputEvent::Motion(m) => m.seq = m.seq.wrapping_add(start),
                InputEvent::Button(b) => b.seq = b.seq.wrapping_add(start),
                InputEvent::Wheel(w) => w.seq = w.seq.wrapping_add(start),
                InputEvent::Gap(_) => {}
            }
        }
        let run = run_live(&v.cfg, &stream, &cutoffs_at(240, stream.end_ns), &[]);
        let rep = replay(&v.cfg, log_of(&run, layout(6), &[]));
        assert!(
            rep.identical(),
            "{start}: {:?} {:?}",
            rep.error,
            rep.mismatch
        );
    }
}

/// The recorder may log an event a group before the frame that drains it;
/// the driving source's undrained events carry over.
#[test]
fn driving_events_carry_over_groups() {
    for v in variants() {
        let stream = record_stream(&v);
        let run = run_live(
            &v.cfg,
            &stream,
            &cutoffs_random(v.bot_seed, 400, 30_000, stream.end_ns),
            &[],
        );
        for per_group in [1, 7, 40] {
            let mut groups = log_of(&run, layout(per_group), &[]);
            for gi in 1..groups.len() {
                let pos = groups[gi]
                    .iter()
                    .position(|i| matches!(i, Item::Event(InputEvent::Motion(m)) if m.source == v.cfg.driving_source));
                if let Some(p) = pos {
                    let item = groups[gi].remove(p);
                    groups[gi - 1].insert(0, item);
                }
            }
            let rep = replay(&v.cfg, groups);
            assert!(
                rep.identical(),
                "{}/{per_group}: {:?} {:?}",
                v.name,
                rep.error,
                rep.mismatch
            );
            assert_eq!((rep.state_hash, rep.undrained), (run.sim.state_hash(), 0));
        }
    }
}

fn base_log() -> (kf_sim::SimConfig, Vec<Vec<Item>>) {
    let v = &variants()[0];
    let stream = record_stream(v);
    let run = run_live(&v.cfg, &stream, &cutoffs_at(144, stream.end_ns), &[]);
    (v.cfg.clone(), log_of(&run, layout(10), &[]))
}

/// The n-th logged record of a kind (0 control, 1 camera, 2 target, 3 shot).
fn nth(groups: &mut [Vec<Item>], kind: usize, n: usize) -> (usize, usize) {
    let mut k = 0;
    for (gi, g) in groups.iter().enumerate() {
        for (ii, item) in g.iter().enumerate() {
            let hit = matches!(
                (kind, item),
                (0, Item::Control(_))
                    | (1, Item::Logged(SimRecord::Camera(_)))
                    | (2, Item::Logged(SimRecord::Target(_)))
                    | (3, Item::Logged(SimRecord::Shot(_)))
            );
            if hit {
                if k == n {
                    return (gi, ii);
                }
                k += 1;
            }
        }
    }
    panic!("no record {n} of kind {kind}")
}

#[test]
fn reports_mismatches_of_every_kind() {
    let (cfg, base) = base_log();
    type Edit = fn(&mut Vec<Vec<Item>>);
    let cases: [(&str, u64, Edit); 6] = [
        ("sim_camera", 4, |g| {
            let (a, b) = nth(g, 1, 4);
            if let Item::Logged(SimRecord::Camera(c)) = &mut g[a][b] {
                c.yaw_units += 1;
            }
        }),
        ("sim_target", 3, |g| {
            let (a, b) = nth(g, 2, 3);
            if let Item::Logged(SimRecord::Target(t)) = &mut g[a][b] {
                t.radius_m = f64::from_bits(t.radius_m.to_bits() ^ 1);
            }
        }),
        ("sim_control", 0, |g| {
            let (a, b) = nth(g, 0, 0);
            if let Item::Control(c) = &mut g[a][b] {
                c.tick += 1;
            }
        }),
        ("sim_shot", 2, |g| {
            let (a, b) = nth(g, 3, 2);
            if let Item::Logged(SimRecord::Shot(s)) = &mut g[a][b] {
                s.ang_err_rad = f64::from_bits(s.ang_err_rad.to_bits() ^ 1);
            }
        }),
        ("sim_target", 5, |g| {
            let (a, b) = nth(g, 2, 5);
            g[a].remove(b); // a missing record
        }),
        ("sim_shot", 1, |g| {
            let (a, b) = nth(g, 3, 0);
            let extra = g[a][b].clone();
            g[a].insert(b + 1, extra); // an extra record
        }),
    ];
    for (kind, index, edit) in cases {
        let mut g = base.clone();
        edit(&mut g);
        let rep = replay(&cfg, g);
        let m = rep
            .mismatch
            .unwrap_or_else(|| panic!("{kind} {index}: no mismatch ({:?})", rep.error));
        assert_eq!((m.kind, m.index), (kind, index));
    }
    // The last sim_target of a group missing: the replay produced one more
    // than the log ("log has fewer" direction).
    let is_target = |x: &&Item| matches!(x, Item::Logged(SimRecord::Target(_)));
    let mut g = base.clone();
    let gi = (1..g.len())
        .find(|&i| g[i].iter().filter(is_target).count() >= 2)
        .unwrap();
    let before = g[..gi].iter().flatten().filter(is_target).count();
    let in_group = g[gi].iter().filter(is_target).count();
    let last = g[gi].iter().rposition(|x| is_target(&x)).unwrap();
    g[gi].remove(last);
    let m = replay(&cfg, g).mismatch.expect("mismatch");
    assert_eq!(
        (m.kind, m.index),
        ("sim_target", (before + in_group - 1) as u64)
    );
    assert!(m.logged.is_none() && m.replayed.is_some());
}

#[test]
fn malformed_logs_are_not_replayable() {
    use kf_core::SimControl;
    let (cfg, base) = base_log();
    fn frame_of(g: &[Item]) -> kf_core::Frame {
        g.iter()
            .find_map(|i| match i {
                Item::Frame(f) => Some(*f),
                _ => None,
            })
            .unwrap()
    }
    fn drain_pos(g: &[Item]) -> usize {
        g.iter().position(|i| matches!(i, Item::Drain(_))).unwrap()
    }
    type Edit = Box<dyn Fn(&mut Vec<Vec<Item>>)>;
    let ctl = |action: u8, phase: u8| -> Edit {
        Box::new(move |g: &mut Vec<Vec<Item>>| {
            let f = frame_of(&g[3]);
            g[3].push(Item::Control(SimControl {
                tick: f.first_tick,
                t_ns: f.t_input_cutoff_ns,
                frame: f.frame,
                action,
                phase,
                reason: 0,
                new_source: None,
            }));
        })
    };
    let cases: Vec<(&str, Edit)> = vec![
        ("UnknownAction", ctl(8, 0)),
        // switch_source while the trial runs unpaused (and without a source).
        ("BadSwitch", ctl(act::SWITCH_SOURCE, 0)),
        ("phase 2", ctl(act::PAUSE, 2)),
        (
            "first_tick",
            Box::new(|g| {
                if let Some(Item::Frame(f)) = g[3].iter_mut().find(|i| matches!(i, Item::Frame(_)))
                {
                    f.first_tick += 1;
                }
            }),
        ),
        (
            "TooManyTicks",
            Box::new(|g| {
                if let Some(Item::Frame(f)) = g[3].iter_mut().find(|i| matches!(i, Item::Frame(_)))
                {
                    f.ticks = 1001;
                }
            }),
        ),
        (
            "outside its group",
            Box::new(|g| {
                let p = drain_pos(&g[3]);
                let other = frame_of(&g[4]).frame;
                if let Item::Drain(d) = &mut g[3][p] {
                    d.frame = other;
                }
            }),
        ),
        (
            "two drains",
            Box::new(|g| {
                let p = drain_pos(&g[3]);
                let d = g[3][p].clone();
                g[3].insert(p, d);
            }),
        ),
        (
            "drain of source 9",
            Box::new(|g| {
                let p = drain_pos(&g[3]);
                if let Item::Drain(d) = &mut g[3][p] {
                    d.source = 9;
                }
            }),
        ),
        (
            "FrameNumber",
            Box::new(|g| {
                // Drop a frame (its ticks go to the next one, so ticks stay
                // continuous): the frame number jumps.
                let f = frame_of(&g[3]);
                g[3].retain(|i| match i {
                    Item::Frame(x) => x.frame != f.frame,
                    Item::Drain(d) => d.frame != f.frame,
                    Item::Control(c) => c.frame != f.frame,
                    _ => true,
                });
                for i in g[3].iter_mut() {
                    if let Item::Frame(x) = i
                        && x.frame == f.frame + 1
                    {
                        x.first_tick = f.first_tick;
                        x.ticks += f.ticks;
                    }
                }
            }),
        ),
        (
            "seq repeats or goes back",
            Box::new(|g| {
                let e = g[3]
                    .iter()
                    .find(|i| matches!(i, Item::Event(InputEvent::Motion(_))))
                    .unwrap()
                    .clone();
                g[3].insert(0, e);
            }),
        ),
        (
            "seq repeats or goes back",
            Box::new(|g| {
                // A late record: group 2's first motion again in group 4.
                let e = g[2]
                    .iter()
                    .find(|i| matches!(i, Item::Event(InputEvent::Motion(_))))
                    .unwrap()
                    .clone();
                g[4].insert(0, e);
            }),
        ),
    ];
    for (want, edit) in cases {
        let mut g = base.clone();
        edit(&mut g);
        let rep = replay(&cfg, g);
        assert!(
            rep.error.as_deref().is_some_and(|e| e.contains(want)),
            "{want}: error {:?}, mismatch {:?}",
            rep.error,
            rep.mismatch
        );
    }
}
