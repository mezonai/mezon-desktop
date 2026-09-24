// Copyright 2026 Mezon contributors. Licensed under Apache-2.0.

#[derive(Debug, PartialEq)]
pub(crate) enum FrameDuration {
    Minimum,
    Maximum,
    Exact(u32),
}

pub(crate) fn select_frame_duration(
    requested: u32,
    min_fps: f64,
    max_fps: f64,
) -> Option<FrameDuration> {
    if requested == 0
        || requested > i32::MAX as u32
        || !min_fps.is_finite()
        || !max_fps.is_finite()
        || min_fps <= 0.0
        || min_fps > max_fps
    {
        return None;
    }
    // supported_formats exposes native endpoints as truncated integer FPS.
    // Preserve their exact native durations rather than inventing 29 FPS
    // for a camera whose fixed rate is 29.97 FPS.
    if requested == min_fps as u32 {
        return Some(FrameDuration::Maximum);
    }
    if requested == max_fps as u32 {
        return Some(FrameDuration::Minimum);
    }
    if (min_fps..=max_fps).contains(&f64::from(requested)) {
        return Some(FrameDuration::Exact(requested));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{select_frame_duration, FrameDuration::*};

    #[test]
    fn accepts_minimum_and_interior_rates_not_just_maximum() {
        assert_eq!(select_frame_duration(15, 15.0, 30.0), Some(Maximum));
        assert_eq!(select_frame_duration(24, 15.0, 30.0), Some(Exact(24)));
        assert_eq!(select_frame_duration(30, 15.0, 30.0), Some(Minimum));
    }

    #[test]
    fn preserves_fractional_native_endpoints() {
        assert_eq!(select_frame_duration(29, 29.97, 29.97), Some(Maximum));
        assert_eq!(select_frame_duration(29, 15.0, 29.97), Some(Minimum));
        assert_eq!(select_frame_duration(30, 29.97, 29.97), None);
    }

    #[test]
    fn rejects_unsupported_and_invalid_rates() {
        for fps in [0, 14, 31, u32::MAX] {
            assert_eq!(select_frame_duration(fps, 15.0, 30.0), None);
        }
        for (min, max) in [
            (0.0, 30.0),
            (30.0, 15.0),
            (f64::NAN, 30.0),
            (15.0, f64::INFINITY),
        ] {
            assert_eq!(select_frame_duration(24, min, max), None);
        }
    }
}
