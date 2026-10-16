use crate::model::AutomationPoint;
use std::sync::Arc;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Curve {
    Linear,
    Smooth,
    EaseIn,
    EaseOut,
    Hold,
}
impl Curve {
    fn from_name(name: &str) -> Self {
        match name {
            "smooth" => Self::Smooth,
            "easeIn" => Self::EaseIn,
            "easeOut" => Self::EaseOut,
            "hold" => Self::Hold,
            _ => Self::Linear,
        }
    }
}
fn fraction(curve: Curve, f: f32) -> f32 {
    let f = f.clamp(0., 1.);
    match curve {
        Curve::Smooth => f * f * (3. - 2. * f),
        Curve::EaseIn => f * f,
        Curve::EaseOut => 1. - (1. - f) * (1. - f),
        Curve::Hold => {
            if f < 1. {
                0.
            } else {
                1.
            }
        }
        Curve::Linear => f,
    }
}
/// The same endpoint arithmetic for saved lanes and callback gesture ramps.
pub(crate) fn interpolate(a: f32, b: f32, f: f32, decibels: bool) -> f32 {
    if a == b || f <= 0. {
        return a;
    }
    if f >= 1. {
        return b;
    }
    if decibels {
        let lo = 20. * (a * 1.5).max(0.0001).log10();
        let hi = 20. * (b * 1.5).max(0.0001).log10();
        (10_f32.powf((lo * (1. - f) + hi * f) / 20.) / 1.5).clamp(0., 1.)
    } else {
        a * (1. - f) + b * f
    }
}
#[derive(Clone, Copy, Debug)]
struct PreparedPoint {
    frame: u64,
    value: f32,
    curve_start: f32,
    curve_end: f32,
    curve: Curve,
}
/// Immutable playback-only lane. Authored strings/unknown metadata remain in the
/// session model; checkpoints share compact points rather than cloning that model.
#[derive(Clone, Debug)]
pub struct Lane {
    points: Arc<[PreparedPoint]>,
    baseline: f32,
    decibels: bool,
}
impl Lane {
    pub fn new(points: &[AutomationPoint], baseline: f32, decibels: bool) -> Self {
        let mut points: Vec<_> = points
            .iter()
            .map(|p| PreparedPoint {
                frame: p.frame,
                value: p.value,
                curve_start: p.curve_start,
                curve_end: p.curve_end,
                curve: Curve::from_name(&p.curve),
            })
            .collect();
        points.sort_by_key(|p| p.frame);
        let mut unique: Vec<PreparedPoint> = Vec::with_capacity(points.len());
        for p in points {
            if unique
                .last()
                .map(|last| last.frame == p.frame)
                .unwrap_or(false)
            {
                unique.pop();
            }
            unique.push(p);
        }
        Self {
            points: unique.into(),
            baseline: if baseline.is_finite() {
                baseline.clamp(0., 1.)
            } else {
                0.5
            },
            decibels,
        }
    }
    pub(crate) fn point_storage_bound(count: usize) -> usize {
        count * std::mem::size_of::<PreparedPoint>() + 2 * std::mem::size_of::<usize>()
    }
    pub fn has_points(&self) -> bool {
        !self.points.is_empty()
    }
    pub(crate) fn decibels(&self) -> bool {
        self.decibels
    }
    pub fn value_signed(&self, frame: i64) -> f32 {
        if frame < 0 {
            self.baseline
        } else {
            self.value(frame as u64)
        }
    }
    pub fn value(&self, frame: u64) -> f32 {
        let i = self.points.partition_point(|p| p.frame <= frame);
        if i == 0 {
            return self.baseline;
        }
        let a = &self.points[i - 1];
        if i == self.points.len() {
            return a.value;
        }
        let b = &self.points[i];
        let f = (frame - a.frame) as f32 / (b.frame - a.frame) as f32;
        let f = if a.curve == Curve::Linear {
            f
        } else {
            let lo = fraction(a.curve, a.curve_start);
            let hi = fraction(a.curve, a.curve_end);
            if (hi - lo).abs() > 0.000001 {
                (fraction(a.curve, a.curve_start + (a.curve_end - a.curve_start) * f) - lo)
                    / (hi - lo)
            } else {
                f
            }
        };
        interpolate(a.value, b.value, f, self.decibels)
    }
}
