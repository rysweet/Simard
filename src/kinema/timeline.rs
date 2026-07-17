//! Keyframe sampling and interpolation.
//!
//! Given an object's keyframes and a time `t`, produce the interpolated
//! transform. Times outside the keyframe range clamp to the nearest keyframe
//! (hold). Between keyframes we use smoothstep easing for natural motion.

use super::brief::Keyframe;

/// An interpolated transform at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub opacity: f64,
}

/// Smoothstep easing on `[0, 1]`.
fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Sample the transform at time `t`. Keyframes need not be pre-sorted.
///
/// # Panics
/// Never panics; an empty keyframe slice is treated as the identity transform.
pub fn sample(keyframes: &[Keyframe], t: f64) -> Sample {
    let identity = Sample {
        x: 0.5,
        y: 0.5,
        scale: 1.0,
        opacity: 1.0,
    };
    if keyframes.is_empty() {
        return identity;
    }
    // Work on a time-sorted copy so out-of-order briefs still animate sanely.
    let mut sorted: Vec<&Keyframe> = keyframes.iter().collect();
    sorted.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

    let first = sorted[0];
    if t <= first.t {
        return kf_sample(first);
    }
    let last = sorted[sorted.len() - 1];
    if t >= last.t {
        return kf_sample(last);
    }

    // Find the bracketing pair.
    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t >= a.t && t <= b.t {
            let span = b.t - a.t;
            let raw = if span > 1e-9 { (t - a.t) / span } else { 0.0 };
            let e = ease(raw);
            return Sample {
                x: lerp(a.x, b.x, e),
                y: lerp(a.y, b.y, e),
                scale: lerp(a.scale, b.scale, e),
                opacity: lerp(a.opacity, b.opacity, e),
            };
        }
    }
    kf_sample(last)
}

fn kf_sample(kf: &Keyframe) -> Sample {
    Sample {
        x: kf.x,
        y: kf.y,
        scale: kf.scale,
        opacity: kf.opacity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(t: f64, x: f64, y: f64) -> Keyframe {
        Keyframe {
            t,
            x,
            y,
            scale: 1.0,
            opacity: 1.0,
        }
    }

    #[test]
    fn empty_is_identity() {
        let s = sample(&[], 0.5);
        assert_eq!(s.x, 0.5);
        assert_eq!(s.scale, 1.0);
    }

    #[test]
    fn clamps_before_and_after() {
        let kfs = [kf(1.0, 0.2, 0.3), kf(3.0, 0.8, 0.9)];
        assert_eq!(sample(&kfs, 0.0).x, 0.2);
        assert_eq!(sample(&kfs, 5.0).x, 0.8);
    }

    #[test]
    fn midpoint_is_between_endpoints() {
        let kfs = [kf(0.0, 0.0, 0.0), kf(2.0, 1.0, 1.0)];
        let mid = sample(&kfs, 1.0);
        // Smoothstep(0.5) == 0.5, so the midpoint is exactly halfway.
        assert!((mid.x - 0.5).abs() < 1e-9);
        assert!((mid.y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn eases_non_linearly_off_midpoint() {
        let kfs = [kf(0.0, 0.0, 0.0), kf(1.0, 1.0, 0.0)];
        let quarter = sample(&kfs, 0.25).x;
        // Smoothstep at 0.25 is < 0.25 (ease-in).
        assert!(quarter < 0.25);
    }

    #[test]
    fn handles_unsorted_keyframes() {
        let kfs = [kf(2.0, 1.0, 1.0), kf(0.0, 0.0, 0.0)];
        assert_eq!(sample(&kfs, 0.0).x, 0.0);
        assert_eq!(sample(&kfs, 2.0).x, 1.0);
    }

    #[test]
    fn coincident_keyframe_times_do_not_divide_by_zero() {
        let kfs = [kf(1.0, 0.0, 0.0), kf(1.0, 1.0, 1.0)];
        let s = sample(&kfs, 1.0);
        assert!(s.x.is_finite());
    }
}
