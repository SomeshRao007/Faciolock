//! Synthetic IR captures, generated rather than recorded.
//!
//! Public, not `#[cfg(test)]`, because this is the **committed CI corpus** and not just
//! test scaffolding. The real Phase 0 recordings are 20 MB of the operator's face and
//! are gitignored, and a Debian buildd has no network to fetch them with — so a test
//! that depends on them does not run in the archive, which is where this project is
//! going (wiki/08 Q6). Synthetic frames build offline, in a clean chroot, in kilobytes.
//!
//! **What this can and cannot prove.** It proves the *logic*: that a curved reflector
//! scores differently from a flat one, that a single contaminated pair sinks a burst,
//! that pairing never reuses a frame. It cannot prove a *threshold* — real sensor noise,
//! AGC droop and the actual reflectance of skin against paper are not modelled here and
//! could not be. Thresholds are calibrated against the local recordings and nothing else.

/// Default synthetic frame size. Small on purpose: these run in unit tests.
pub const W: usize = 64;
pub const H: usize = 64;

/// A uniform frame.
pub fn blank(w: usize, h: usize, v: u8) -> Vec<u8> {
    vec![v; w * h]
}

/// A flat reflector: uniform response everywhere, which is what a printed photo is.
pub fn flat(w: usize, h: usize, v: u8) -> Vec<u8> {
    blank(w, h, v)
}

/// A domed reflector: brightest at the centre, falling off radially — a face lit by an
/// emitter beside the lens.
pub fn domed(w: usize, h: usize, peak: u8, floor: u8) -> Vec<u8> {
    let mut f = vec![0u8; w * h];
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let max_r = (cx * cx + cy * cy).sqrt();
    for y in 0..h {
        for x in 0..w {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let t = (dx * dx + dy * dy).sqrt() / max_r; // 0 at centre, 1 at corner
            let v = peak as f64 - t * (peak as f64 - floor as f64);
            f[y * w + x] = v.clamp(0.0, 255.0) as u8;
        }
    }
    f
}

/// Concatenate frames into the back-to-back planar layout a `.gray` recording uses.
pub fn planar(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.concat()
}

/// `pairs` repetitions of an ON frame followed by an OFF frame.
pub fn strobe(on: &[u8], off: &[u8], pairs: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity((on.len() + off.len()) * pairs);
    for _ in 0..pairs {
        out.extend_from_slice(on);
        out.extend_from_slice(off);
    }
    out
}

/// A strobe burst in which exactly one OFF frame is contaminated.
///
/// This is [B9](../../../wiki/11-bug-log.md) in a test fixture. A pulsed IR source idles
/// between packets, so most of its OFF frames read as genuinely dark and only some are
/// bright — 7 of 42 pairs looked perfectly clean in the real recording. Under a per-pair
/// or k-of-n rule that is a 1-in-36 bypass at the two-pair capture budget. Under
/// conjunctive aggregation this burst must fail on the strength of the one bad pair.
pub fn strobe_one_contaminated(
    on: &[u8],
    off: &[u8],
    contaminated_off: &[u8],
    pairs: usize,
    bad_index: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((on.len() + off.len()) * pairs);
    for i in 0..pairs {
        out.extend_from_slice(on);
        out.extend_from_slice(if i == bad_index { contaminated_off } else { off });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domed_is_brighter_at_the_centre_than_at_the_corner() {
        let f = domed(W, H, 60, 10);
        let centre = f[(H / 2) * W + W / 2];
        assert!(centre > f[0], "centre {centre} vs corner {}", f[0]);
        assert!(centre > f[W - 1]);
    }

    #[test]
    fn strobe_alternates_and_has_the_right_length() {
        let on = blank(W, H, 60);
        let off = blank(W, H, 0);
        let burst = strobe(&on, &off, 5);
        assert_eq!(burst.len(), W * H * 10);
        assert_eq!(burst[0], 60);
        assert_eq!(burst[W * H], 0);
    }

    #[test]
    fn exactly_one_pair_is_contaminated() {
        let on = blank(W, H, 60);
        let off = blank(W, H, 0);
        let bad = blank(W, H, 30);
        let burst = strobe_one_contaminated(&on, &off, &bad, 4, 2);
        let offs: Vec<u8> = (0..4).map(|i| burst[(2 * i + 1) * W * H]).collect();
        assert_eq!(offs, vec![0, 0, 30, 0]);
    }
}
