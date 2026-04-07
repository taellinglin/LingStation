//! CLAP parameter values are plain (per-parameter range). LingStation stores automation as normalized 0..1.

const SPAN_EPS: f64 = 1e-12;

/// Map host-normalized `[0,1]` to plugin plain value using CLAP `min_value` / `max_value`.
pub fn normalized_to_plain(norm: f64, min: f64, max: f64) -> Option<f64> {
    if !norm.is_finite() || !min.is_finite() || !max.is_finite() {
        return None;
    }
    let norm = norm.clamp(0.0, 1.0);
    let span = max - min;
    if span.abs() < SPAN_EPS {
        Some(min)
    } else {
        Some(min + norm * span)
    }
}

/// Map plugin plain value to host-normalized `[0,1]` using CLAP `min_value` / `max_value`.
pub fn plain_to_normalized(plain: f64, min: f64, max: f64) -> Option<f64> {
    if !plain.is_finite() || !min.is_finite() || !max.is_finite() {
        return None;
    }
    let span = max - min;
    if span.abs() < SPAN_EPS {
        Some(0.5)
    } else {
        Some(((plain - min) / span).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_unit_range() {
        for n in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            let p = normalized_to_plain(n, 0.0, 1.0).unwrap();
            let back = plain_to_normalized(p, 0.0, 1.0).unwrap();
            assert!((back - n).abs() < 1e-9, "n={n} p={p} back={back}");
        }
    }

    #[test]
    fn scaled_range() {
        let plain = normalized_to_plain(0.5, 20.0, 120.0).unwrap();
        assert!((plain - 70.0).abs() < 1e-9);
        assert!((plain_to_normalized(70.0, 20.0, 120.0).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn decreasing_range() {
        let p0 = normalized_to_plain(0.0, 100.0, 0.0).unwrap();
        let p1 = normalized_to_plain(1.0, 100.0, 0.0).unwrap();
        assert!((p0 - 100.0).abs() < 1e-9);
        assert!((p1 - 0.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_span_returns_mid_normalized() {
        assert!((plain_to_normalized(42.0, 10.0, 10.0).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn degenerate_span_plain_from_norm() {
        assert!((normalized_to_plain(0.25, 7.0, 7.0).unwrap() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_non_finite() {
        assert!(normalized_to_plain(f64::NAN, 0.0, 1.0).is_none());
        assert!(normalized_to_plain(0.5, f64::INFINITY, 1.0).is_none());
        assert!(plain_to_normalized(0.5, 0.0, f64::NAN).is_none());
    }

    #[test]
    fn clamps_normalized_output() {
        assert!((plain_to_normalized(1000.0, 0.0, 1.0).unwrap() - 1.0).abs() < 1e-9);
        assert!((plain_to_normalized(-5.0, 0.0, 1.0).unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn clamps_normalized_input() {
        let p = normalized_to_plain(1.5, 0.0, 10.0).unwrap();
        assert!((p - 10.0).abs() < 1e-9);
        let p2 = normalized_to_plain(-1.0, 0.0, 10.0).unwrap();
        assert!((p2 - 0.0).abs() < 1e-9);
    }
}
