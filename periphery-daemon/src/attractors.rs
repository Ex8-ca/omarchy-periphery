//! The discrete attractors that windows snap to when dragged past the screen edge.
//!
//! We don't actually want snapping. We want *continuous* resize, with the
//! discrete attractors just defining where the size "wants to settle" once
//! the user releases the mouse. The further past the edge they pull, the
//! smaller the attractor they pick.
//!
//! Sizes are expressed as a fraction of the monitor's *shorter* dimension.

/// The available attractors, from largest to smallest. Jenson calls these "the
/// modes of attention" in his prior talks — calm, glance, periphery, ambient.
pub const ATTRACTORS: &[f32] = &[
    1.00,  // full screen (home)
    0.50,  // half (split)
    0.33,  // third (companion)
    0.25,  // quarter (peek)
    0.15,  // small (periphery)
    0.03,  // icon (ambient)
];

/// Given how many "periphery-units" the user has dragged past the edge
/// (each unit = one monitor dimension), return the attractor closest to
/// the current size.
pub fn pick_attractor(drag_units: f32) -> f32 {
    // drag_units is non-negative; the further past the edge, the smaller.
    // Clamp to [0, len(ATTRACTORS)-1].
    let n = (ATTRACTORS.len() - 1) as f32;
    let t = drag_units.clamp(0.0, n);
    let idx = t.round() as usize;
    ATTRACTORS[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attractors_in_decreasing_order() {
        for w in ATTRACTORS.windows(2) {
            assert!(w[0] > w[1], "attractors must decrease: {:?}", ATTRACTORS);
        }
    }

    #[test]
    fn pick_attractor_zero_is_full_screen() {
        assert_eq!(pick_attractor(0.0), 1.00);
    }

    #[test]
    fn pick_attractor_at_max_is_icon() {
        assert_eq!(pick_attractor(100.0), 0.03);
    }

    #[test]
    fn pick_attractor_monotonically_decreases() {
        let mut last = 2.0;
        for u in [0.0, 0.4, 1.0, 1.6, 2.4, 3.0, 3.6, 4.0, 5.0] {
            let v = pick_attractor(u);
            assert!(v <= last, "non-monotonic at u={u}: {} > {}", v, last);
            last = v;
        }
    }
}