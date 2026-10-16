//! Worker-only finalization of actual callback ramps. No client clock, desired
//! endpoint, network, device, or mutable authored model participates here.
use soundroom_engine::{automation::Lane, model::AutomationPoint};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_CAPTURED_RAMPS: usize = 65_536;
pub const MAX_WRITTEN_POINTS: usize = 4_096;
pub const MAX_LANE_POINTS: usize = 100_000;
pub const MAX_DURATION_FRAMES: u64 = 28_800_000;
const MAX_SIMPLIFIER_WORK: usize = 8_000_000;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedRamp {
    pub frame: u64,
    pub start: f32,
    pub target: f32,
    pub duration: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Released {
    /// The last sample actually evaluated by the override, never its desired end.
    pub last_frame: u64,
    pub last_value: f32,
    /// First sample that evaluates the untouched original Read lane again.
    pub next_read_frame: u64,
}
#[derive(Clone, Copy, Debug)]
pub enum ErrorBound {
    /// Conservative normalized tolerance; callers derive it from physical/log
    /// descriptor Lipschitz bounds, never from a UI pixel distance.
    Normalized(f32),
    Decibels(f32),
}
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    InvalidCapture,
    CaptureLimit,
    PointLimit,
    WorkLimit,
    Cancelled,
}
/// Full receipts remain available to the caller for a reviewable draft on error.
pub fn finalize(
    original: &[AutomationPoint],
    baseline: f32,
    decibels: bool,
    ramps: &[AppliedRamp],
    released: Option<Released>,
    bound: ErrorBound,
    cancel: &AtomicBool,
) -> Result<Vec<AutomationPoint>, Failure> {
    if ramps.is_empty() {
        return Ok(original.to_vec());
    }
    if ramps.len() > MAX_CAPTURED_RAMPS || original.len() > MAX_LANE_POINTS {
        return Err(Failure::CaptureLimit);
    }
    if !valid_value(baseline)
        || original
            .iter()
            .map(|p| p.frame)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != original.len()
        || original.iter().any(|p| {
            !valid_value(p.value)
                || p.frame > soundroom_engine::model::MAX_FRAME
                || !valid_value(p.curve_start)
                || !valid_value(p.curve_end)
                || p.curve_start > p.curve_end
                || !["linear", "smooth", "easeIn", "easeOut", "hold"].contains(&p.curve.as_str())
        })
        || ramps.iter().any(|r| {
            !valid_value(r.start)
                || !valid_value(r.target)
                || r.duration == 0
                || r.duration > 480
                || r.frame.checked_add(r.duration).is_none()
        })
        || ramps.windows(2).any(|r| r[0].frame >= r[1].frame)
    {
        return Err(Failure::InvalidCapture);
    }
    let released = released.ok_or(Failure::InvalidCapture)?;
    if !valid_value(released.last_value)
        || released.last_frame < ramps.last().unwrap().frame
        || released.next_read_frame
            != released
                .last_frame
                .checked_add(1)
                .ok_or(Failure::InvalidCapture)?
        || released.next_read_frame > soundroom_engine::model::MAX_FRAME
        || released.next_read_frame - ramps[0].frame > MAX_DURATION_FRAMES
    {
        return Err(Failure::InvalidCapture);
    }
    let normalized_error = match bound {
        ErrorBound::Normalized(v) if v.is_finite() && v > 0. && v <= 1. => v,
        ErrorBound::Decibels(v) if decibels && v.is_finite() && v > 0. && v <= 1. => v,
        _ => return Err(Failure::InvalidCapture),
    };
    let tolerance = if decibels {
        match bound {
            ErrorBound::Decibels(v) => v,
            _ => normalized_error / (std::f32::consts::LN_10 / 20.),
        }
    } else {
        normalized_error
    };
    // Two points per ramp plus its held end; all insertion is bounded before it
    // becomes an authored point. Values from a partial ramp are actual receipts.
    let mut raw = BTreeMap::<u64, f32>::new();
    for (index, ramp) in ramps.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        let until = ramps
            .get(index + 1)
            .map_or(released.last_frame, |next| next.frame);
        raw.insert(ramp.frame, ramp.start);
        let end = (ramp.frame + ramp.duration).min(until);
        raw.insert(
            end,
            interpolate(
                ramp.start,
                ramp.target,
                (end - ramp.frame) as f32 / ramp.duration as f32,
                decibels,
            ),
        );
    }
    // A subsequent receipt owns the next boundary. Never retain an old planned
    // endpoint past that boundary, or extrapolate a target past a stopped ramp.
    raw.retain(|frame, _| *frame <= released.last_frame);
    raw.insert(released.last_frame, released.last_value);
    let first = ramps[0].frame;
    let read = Lane::new(original, baseline, decibels);
    let read_value = read.value(released.next_read_frame);
    raw.insert(released.next_read_frame, read_value);
    let raw: Vec<_> = raw.into_iter().collect();
    let reduced = simplify(&raw, decibels, tolerance, cancel)?;
    // Keep the first untouched Read point out of the written-point quota.
    if reduced.len().saturating_sub(1) > MAX_WRITTEN_POINTS {
        return Err(Failure::PointLimit);
    }
    let mut result = canonical(original);
    if first > 0 {
        insert_boundary(&mut result, first - 1, baseline, decibels);
    }
    insert_boundary(&mut result, released.next_read_frame, baseline, decibels);
    result.retain(|point| point.frame < first || point.frame >= released.next_read_frame);
    result.extend(
        reduced
            .into_iter()
            .filter(|(frame, _)| *frame < released.next_read_frame)
            .map(|(frame, value)| point(frame, value)),
    );
    result.sort_by_key(|p| p.frame);
    if result.len() > MAX_LANE_POINTS {
        return Err(Failure::PointLimit);
    }
    Ok(result)
}
fn valid_value(value: f32) -> bool {
    value.is_finite() && (0. ..=1.).contains(&value)
}
fn point(frame: u64, value: f32) -> AutomationPoint {
    AutomationPoint {
        frame,
        value,
        curve: "linear".into(),
        curve_start: 0.,
        curve_end: 1.,
        extra: BTreeMap::new(),
    }
}
fn interpolate(a: f32, b: f32, f: f32, decibels: bool) -> f32 {
    if a == b {
        return a;
    }
    if f <= 0. {
        return a;
    }
    if f >= 1. {
        return b;
    }
    if decibels {
        (10_f32.powf((db(a) * (1. - f) + db(b) * f) / 20.) / 1.5).clamp(0., 1.)
    } else {
        a * (1. - f) + b * f
    }
}
fn db(v: f32) -> f32 {
    20. * (v * 1.5).max(0.0001).log10()
}
fn canonical(original: &[AutomationPoint]) -> Vec<AutomationPoint> {
    let mut indexed = BTreeMap::new();
    for p in original {
        indexed.insert(p.frame, p.clone());
    }
    indexed.into_values().collect()
}
/// Splitting retains the original outgoing fractional curve domain. Before the
/// first authored point Read is constant, so the new point MUST hold that value.
fn insert_boundary(points: &mut Vec<AutomationPoint>, frame: u64, baseline: f32, decibels: bool) {
    let index = points.partition_point(|p| p.frame < frame);
    if points.get(index).is_some_and(|p| p.frame == frame) {
        return;
    }
    let value = Lane::new(points, baseline, decibels).value(frame);
    let mut inserted = point(frame, value);
    if index == 0 && !points.is_empty() {
        inserted.curve = "hold".into();
    } else if index > 0 && index < points.len() {
        let next_frame = points[index].frame;
        let lower = &mut points[index - 1];
        let middle = lower.curve_start
            + (lower.curve_end - lower.curve_start) * (frame - lower.frame) as f32
                / (next_frame - lower.frame) as f32;
        inserted.curve = lower.curve.clone();
        inserted.curve_start = middle;
        inserted.curve_end = lower.curve_end;
        lower.curve_end = middle;
    }
    points.insert(index, inserted);
}
/// Iterative RDP in the lane interpolation domain. Within each original linear
/// ramp the largest transformed error lies at a receipt boundary. Mandatory
/// silence points preserve endpoint floor semantics. Work is explicitly bounded.
fn simplify(
    points: &[(u64, f32)],
    decibels: bool,
    tolerance: f32,
    cancel: &AtomicBool,
) -> Result<Vec<(u64, f32)>, Failure> {
    if points.len() <= 2 {
        return Ok(points.to_vec());
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    if decibels {
        for (i, (_, value)) in points.iter().enumerate() {
            if *value == 0. {
                keep[i] = true;
                if i > 0 {
                    keep[i - 1] = true;
                }
                if i + 1 < points.len() {
                    keep[i + 1] = true;
                }
            }
        }
    }
    let required: Vec<_> = keep
        .iter()
        .enumerate()
        .filter_map(|(i, value)| value.then_some(i))
        .collect();
    let mut stack: Vec<_> = required.windows(2).map(|p| (p[0], p[1])).collect();
    let mut work = 0usize;
    while let Some((a, b)) = stack.pop() {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        let mapped = |v: f32| if decibels { db(v) } else { v };
        let mut worst = tolerance;
        let mut split = None;
        for i in a + 1..b {
            work += 1;
            if work > MAX_SIMPLIFIER_WORK {
                return Err(Failure::WorkLimit);
            }
            if work % 256 == 0 && cancel.load(Ordering::Acquire) {
                return Err(Failure::Cancelled);
            }
            let f = (points[i].0 - points[a].0) as f64 / (points[b].0 - points[a].0) as f64;
            let predicted = mapped(points[a].1) as f64 * (1. - f) + mapped(points[b].1) as f64 * f;
            let error = (mapped(points[i].1) as f64 - predicted).abs() as f32;
            if error > worst {
                worst = error;
                split = Some(i);
            }
        }
        if let Some(i) = split {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    Ok(points
        .iter()
        .zip(keep)
        .filter_map(|(p, keep)| keep.then_some(*p))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(
        original: &[AutomationPoint],
        baseline: f32,
        decibels: bool,
        ramps: &[AppliedRamp],
        end: Released,
    ) -> Vec<AutomationPoint> {
        finalize(
            original,
            baseline,
            decibels,
            ramps,
            Some(end),
            ErrorBound::Normalized(0.000001),
            &AtomicBool::new(false),
        )
        .unwrap()
    }
    #[test]
    fn before_first_point_preserves_constant_read_and_hold_jump() {
        let mut first = point(1000, 0.8);
        first.curve = "smooth".into();
        let original = vec![first, point(2000, 0.2)];
        let result = run(
            &original,
            0.5,
            false,
            &[AppliedRamp {
                frame: 100,
                start: 0.5,
                target: 0.2,
                duration: 100,
            }],
            Released {
                last_frame: 399,
                last_value: 0.5,
                next_read_frame: 400,
            },
        );
        let before = Lane::new(&original, 0.5, false);
        let after = Lane::new(&result, 0.5, false);
        for f in 0..=2200 {
            if !(100..400).contains(&f) {
                assert!(
                    (before.value(f) - after.value(f)).abs() < 0.00001,
                    "Frame {f}"
                );
            }
        }
        assert_eq!(
            result.iter().find(|p| p.frame == 400).unwrap().curve,
            "hold"
        );
    }
    #[test]
    fn nonlinear_subdomains_decibels_and_silence_preserve_outside_samples() {
        for curve in ["linear", "smooth", "easeIn", "easeOut", "hold"] {
            for decibels in [false, true] {
                for (start, end) in [(0., 0.8), (0.2, 0.9), (0.9, 0.)] {
                    let mut first = point(100, start);
                    first.curve = curve.into();
                    first.curve_start = 0.1;
                    first.curve_end = 0.85;
                    let original = vec![first, point(1500, end), point(2000, 0.3)];
                    let lane = Lane::new(&original, 0.5, decibels);
                    let result = run(
                        &original,
                        0.5,
                        decibels,
                        &[AppliedRamp {
                            frame: 500,
                            start: lane.value(500),
                            target: 0.1,
                            duration: 256,
                        }],
                        Released {
                            last_frame: 899,
                            last_value: lane.value(899),
                            next_read_frame: 900,
                        },
                    );
                    let after = Lane::new(&result, 0.5, decibels);
                    for f in 0..2300 {
                        if !(500..900).contains(&f) {
                            assert!(
                                (lane.value(f) - after.value(f)).abs() < 0.00003,
                                "{curve} db={decibels} f={f}: {} vs {}",
                                lane.value(f),
                                after.value(f)
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn partial_ramp_uses_actual_terminal_not_desired_endpoint() {
        let result = run(
            &[],
            0.5,
            false,
            &[AppliedRamp {
                frame: 100,
                start: 0.5,
                target: 1.,
                duration: 256,
            }],
            Released {
                last_frame: 163,
                last_value: 0.5 + 0.5 * 63. / 256.,
                next_read_frame: 164,
            },
        );
        let lane = Lane::new(&result, 0.5, false);
        for f in 100..=163 {
            assert!((lane.value(f) - (0.5 + 0.5 * (f - 100) as f32 / 256.)).abs() < 0.000001);
        }
        assert_eq!(lane.value(164), 0.5);
        assert_eq!(lane.value(1000), 0.5);
        assert!(!result.iter().any(|p| p.value == 1.));
    }
    #[test]
    fn new_applied_receipt_truncates_prior_planned_ramp() {
        let at150 = 0.5 + 0.5 * 50. / 256.;
        let ramps = [
            AppliedRamp {
                frame: 100,
                start: 0.5,
                target: 1.,
                duration: 256,
            },
            AppliedRamp {
                frame: 150,
                start: at150,
                target: 0.1,
                duration: 256,
            },
        ];
        let last = interpolate(at150, 0.1, 100. / 256., false);
        let result = run(
            &[],
            0.5,
            false,
            &ramps,
            Released {
                last_frame: 250,
                last_value: last,
                next_read_frame: 251,
            },
        );
        let lane = Lane::new(&result, 0.5, false);
        for f in 100..=250 {
            let expected = if f < 150 {
                interpolate(0.5, 1., (f - 100) as f32 / 256., false)
            } else {
                interpolate(at150, 0.1, (f - 150) as f32 / 256., false)
            };
            assert!((lane.value(f) - expected).abs() < 0.000001);
        }
    }
    #[test]
    fn begin_without_applied_value_leaves_original_metadata_exact() {
        let mut p = point(30, 0.7);
        p.extra
            .insert("futureProperty".into(), serde_json::json!({"v":1}));
        assert_eq!(
            finalize(
                &[p.clone()],
                0.5,
                false,
                &[],
                None,
                ErrorBound::Normalized(0.001),
                &AtomicBool::new(false)
            )
            .unwrap(),
            vec![p]
        );
    }
    #[test]
    fn ambiguous_duplicate_original_points_are_rejected_without_normalization() {
        let mut first = point(20, 0.2);
        first
            .extra
            .insert("unrelatedMetadata".into(), serde_json::json!("first"));
        let mut second = point(20, 0.8);
        second.curve = "smooth".into();
        second
            .extra
            .insert("unrelatedMetadata".into(), serde_json::json!("second"));
        let original = vec![first, second];
        let untouched = original.clone();
        assert_eq!(
            finalize(
                &original,
                0.5,
                false,
                &[AppliedRamp {
                    frame: 100,
                    start: 0.5,
                    target: 0.2,
                    duration: 256
                }],
                Some(Released {
                    last_frame: 120,
                    last_value: 0.4,
                    next_read_frame: 121
                }),
                ErrorBound::Normalized(0.001),
                &AtomicBool::new(false)
            )
            .unwrap_err(),
            Failure::InvalidCapture
        );
        assert_eq!(original, untouched);
        assert_eq!(
            finalize(
                &original,
                0.5,
                false,
                &[],
                None,
                ErrorBound::Normalized(0.001),
                &AtomicBool::new(false)
            )
            .unwrap(),
            original
        );
    }
    #[test]
    fn cancellation_and_invalid_or_missing_actual_terminal_reject() {
        let ramp = AppliedRamp {
            frame: 30,
            start: 0.5,
            target: 0.7,
            duration: 256,
        };
        assert_eq!(
            finalize(
                &[],
                0.5,
                false,
                &[ramp],
                None,
                ErrorBound::Normalized(0.001),
                &AtomicBool::new(false)
            )
            .unwrap_err(),
            Failure::InvalidCapture
        );
        let end = Released {
            last_frame: 40,
            last_value: 0.5,
            next_read_frame: 41,
        };
        assert_eq!(
            finalize(
                &[],
                0.5,
                false,
                &[ramp],
                Some(end),
                ErrorBound::Normalized(0.001),
                &AtomicBool::new(true)
            )
            .unwrap_err(),
            Failure::Cancelled
        );
    }
    #[test]
    fn frame_zero_and_after_last_edits_preserve_read_tail_and_metadata() {
        let mut p = point(30, 0.7);
        p.extra
            .insert("futureProperty".into(), serde_json::json!(true));
        for start in [0, 100] {
            let original = vec![p.clone()];
            let read = Lane::new(&original, 0.4, false);
            let result = run(
                &original,
                0.4,
                false,
                &[AppliedRamp {
                    frame: start,
                    start: read.value(start),
                    target: 0.2,
                    duration: 20,
                }],
                Released {
                    last_frame: start + 49,
                    last_value: 0.5,
                    next_read_frame: start + 50,
                },
            );
            let lane = Lane::new(&result, 0.4, false);
            for f in 0..300 {
                if !(start..start + 50).contains(&f) {
                    assert!((read.value(f) - lane.value(f)).abs() < 0.000001);
                }
            }
            if start > 30 {
                assert_eq!(
                    result.iter().find(|v| v.frame == 30).unwrap().extra,
                    p.extra
                );
            }
        }
    }
    #[test]
    fn receipt_simplification_stays_within_declared_error_at_every_sample() {
        for decibels in [false, true] {
            let mut ramps = Vec::new();
            let mut value = 0.5;
            for i in 0..80 {
                let target = 0.15 + 0.65 * (i as f32 * 0.1).sin().abs();
                ramps.push(AppliedRamp {
                    frame: 100 + i * 32,
                    start: value,
                    target,
                    duration: 16,
                });
                value = target;
            }
            let end = Released {
                last_frame: 100 + 79 * 32 + 24,
                last_value: value,
                next_read_frame: 100 + 79 * 32 + 25,
            };
            let tolerance = if decibels {
                ErrorBound::Decibels(0.04)
            } else {
                ErrorBound::Normalized(0.002)
            };
            let result = finalize(
                &[],
                0.5,
                decibels,
                &ramps,
                Some(end),
                tolerance,
                &AtomicBool::new(false),
            )
            .unwrap();
            let lane = Lane::new(&result, 0.5, decibels);
            for frame in 100..=end.last_frame {
                let ramp = ramps.iter().rfind(|r| r.frame <= frame).unwrap();
                let expected = interpolate(
                    ramp.start,
                    ramp.target,
                    ((frame - ramp.frame) as f32 / ramp.duration as f32).min(1.),
                    decibels,
                );
                let actual = lane.value(frame);
                let error = if decibels {
                    (db(expected) - db(actual)).abs()
                } else {
                    (expected - actual).abs()
                };
                assert!(
                    error <= if decibels { 0.04001 } else { 0.00201 },
                    "db={decibels} frame={frame} error={error}"
                );
            }
        }
    }
    #[test]
    fn bounded_constant_capture_compresses_but_silence_detail_fails_truthfully() {
        let constant: Vec<_> = (0..MAX_CAPTURED_RAMPS as u64)
            .map(|frame| AppliedRamp {
                frame,
                start: 0.5,
                target: 0.5,
                duration: 256,
            })
            .collect();
        let result = run(
            &[],
            0.5,
            false,
            &constant,
            Released {
                last_frame: MAX_CAPTURED_RAMPS as u64,
                last_value: 0.5,
                next_read_frame: MAX_CAPTURED_RAMPS as u64 + 1,
            },
        );
        assert!(result.len() <= 3);
        let detail: Vec<_> = (0..5000)
            .map(|frame| AppliedRamp {
                frame,
                start: if frame % 2 == 0 { 0. } else { 1. },
                target: 0.,
                duration: 256,
            })
            .collect();
        assert_eq!(
            finalize(
                &[],
                0.5,
                true,
                &detail,
                Some(Released {
                    last_frame: 5000,
                    last_value: 0.,
                    next_read_frame: 5001
                }),
                ErrorBound::Decibels(0.01),
                &AtomicBool::new(false)
            )
            .unwrap_err(),
            Failure::PointLimit
        );
    }
}
