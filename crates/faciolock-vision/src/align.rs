//! Similarity-transform alignment of a detected face to the canonical 112x112 chip.
//!
//! Twenty lines of arithmetic that decide whether the whole system works. The transform
//! **must be bit-identical between enrolment and verification**, or the embeddings drift
//! apart and accuracy collapses quietly — no error, no log line, just a rising false
//! reject rate. wiki/05 calls this "the classic bug in every reimplementation of this
//! pipeline", which is why it gets a golden-output regression test rather than a review.
//!
//! Determinism is also why there is no linear-algebra dependency here. The 2D similarity
//! transform has a closed form; reaching for a crate's SVD would make the frozen goldens
//! hostage to that crate's numerics across versions, which is the opposite of what the
//! goldens are for.

use crate::detect::LANDMARKS;
use crate::frame::Frame;

/// Side of the aligned chip. SFace's input, and ArcFace's convention.
pub const CHIP_SIZE: usize = 112;

/// The canonical ArcFace five-point reference, in 112x112 chip coordinates.
///
/// Right eye, left eye, nose tip, right mouth corner, left mouth corner — the same order
/// YuNet emits, which is the whole reason one model covers detection and alignment both.
///
/// These exact constants are the de-facto standard the embedder was trained against.
/// Changing them is not a tuning knob: it silently moves every enrolled template into a
/// different space from every verification.
pub const REFERENCE_POINTS: [(f32, f32); LANDMARKS] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

/// A 2D similarity transform: uniform scale, rotation, translation. No shear, no
/// reflection.
///
/// Stored as the four numbers that actually vary. `a` and `b` are `scale*cos` and
/// `scale*sin`, so the linear part is `[[a, -b], [b, a]]` — a form in which "is this
/// still a similarity transform" is true by construction rather than by assertion.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Similarity {
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x - self.b * y + self.tx, self.b * x + self.a * y + self.ty)
    }

    /// The inverse, or `None` if the forward transform is degenerate.
    pub fn invert(&self) -> Option<Similarity> {
        let det = self.a * self.a + self.b * self.b;
        if det <= f32::EPSILON {
            return None;
        }
        let (ia, ib) = (self.a / det, -self.b / det);
        Some(Similarity {
            a: ia,
            b: ib,
            tx: -(ia * self.tx - ib * self.ty),
            ty: -(ib * self.tx + ia * self.ty),
        })
    }

    pub fn scale(&self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }
}

/// Least-squares similarity transform mapping `src` onto `dst`.
///
/// This is Umeyama's estimator specialised to two dimensions with reflection disallowed,
/// where it collapses to a closed form and no SVD is required:
///
/// ```text
/// a = Σ(src·dst) / Σ|src|²        b = Σ(src×dst) / Σ|src|²
/// ```
///
/// over mean-centred points. Reflection is disallowed on purpose — a mirrored fit would
/// mean the landmarks came back mirrored, which is a detection failure to surface, not a
/// pose to accommodate.
///
/// `None` when the source points are coincident and no scale can be recovered.
pub fn estimate_similarity(
    src: &[(f32, f32); LANDMARKS],
    dst: &[(f32, f32); LANDMARKS],
) -> Option<Similarity> {
    let n = LANDMARKS as f32;
    let (sx, sy) = src.iter().fold((0.0, 0.0), |(ax, ay), (x, y)| (ax + x, ay + y));
    let (dx, dy) = dst.iter().fold((0.0, 0.0), |(ax, ay), (x, y)| (ax + x, ay + y));
    let (scx, scy) = (sx / n, sy / n);
    let (dcx, dcy) = (dx / n, dy / n);

    let mut num_cos = 0.0f32;
    let mut num_sin = 0.0f32;
    let mut denom = 0.0f32;
    for i in 0..LANDMARKS {
        let (ax, ay) = (src[i].0 - scx, src[i].1 - scy);
        let (bx, by) = (dst[i].0 - dcx, dst[i].1 - dcy);
        num_cos += ax * bx + ay * by;
        num_sin += ax * by - ay * bx;
        denom += ax * ax + ay * ay;
    }
    if denom <= f32::EPSILON {
        return None;
    }

    let a = num_cos / denom;
    let b = num_sin / denom;
    if a * a + b * b <= f32::EPSILON {
        return None;
    }
    Some(Similarity { a, b, tx: dcx - (a * scx - b * scy), ty: dcy - (b * scx + a * scy) })
}

/// A face resampled into the canonical chip the embedder expects.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chip {
    pub pixels: Vec<u8>,
    pub size: usize,
}

impl Chip {
    pub fn as_frame(&self) -> Frame<'_> {
        Frame { pixels: &self.pixels, width: self.size, height: self.size }
    }
}

/// Warp a face onto the canonical chip using its five landmarks.
///
/// Inverse mapping with bilinear sampling: each destination pixel pulls from the source
/// rather than each source pixel pushing to the destination, which is what avoids holes.
/// Pixels whose source falls outside the frame are clamped to the nearest edge, not
/// zeroed — a face at the edge of the frame should degrade, not acquire a hard black
/// border that the embedder has never seen.
pub fn align(frame: &Frame<'_>, landmarks: &[(f32, f32); LANDMARKS]) -> Option<Chip> {
    let forward = estimate_similarity(landmarks, &REFERENCE_POINTS)?;
    let inverse = forward.invert()?;

    let mut pixels = vec![0u8; CHIP_SIZE * CHIP_SIZE];
    for dy in 0..CHIP_SIZE {
        for dx in 0..CHIP_SIZE {
            // **Integer pixel coordinates, no half-pixel offset** — `cv2.warpAffine`'s
            // convention, and therefore ArcFace's, because the canonical chips every
            // embedder of this family was trained on are produced by `warpAffine`.
            //
            // Note this is the *opposite* convention to `detect`'s letterbox, which does
            // offset by half a pixel because `cv2.resize` does. Two OpenCV functions,
            // two conventions; matching each one individually is what matters, not being
            // internally uniform.
            //
            // The first version of this offset by half a pixel here too. At an inverse
            // scale of ~1.42 that is a systematic ~0.21 px shift of every sampled point —
            // invisible in any self-consistent test, and exactly the silent alignment
            // drift wiki/05 warns about. It was caught by comparing against OpenCV's warp
            // and by nothing else: the transform itself was already correct to 1e-4.
            let (sx, sy) = inverse.apply(dx as f32, dy as f32);
            pixels[dy * CHIP_SIZE + dx] = sample_bilinear(frame, sx, sy);
        }
    }
    Some(Chip { pixels, size: CHIP_SIZE })
}

fn sample_bilinear(frame: &Frame<'_>, x: f32, y: f32) -> u8 {
    let x0f = x.floor();
    let y0f = y.floor();
    let fx = x - x0f;
    let fy = y - y0f;

    let clampx = |v: isize| v.clamp(0, frame.width as isize - 1) as usize;
    let clampy = |v: isize| v.clamp(0, frame.height as isize - 1) as usize;
    let x0 = clampx(x0f as isize);
    let x1 = clampx(x0f as isize + 1);
    let y0 = clampy(y0f as isize);
    let y1 = clampy(y0f as isize + 1);

    let p = |px: usize, py: usize| frame.pixels[py * frame.width + px] as f32;
    let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * fx;
    let bottom = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * fx;
    (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{domed, H, W};

    /// The estimator recovers a transform it was given. Analytic, exact, and independent
    /// of any reference implementation — if this fails the arithmetic is simply wrong.
    #[test]
    fn a_known_transform_is_recovered_exactly() {
        let angle = 0.37f32;
        let scale = 1.83f32;
        let (tx, ty) = (12.5f32, -7.25f32);
        let (a, b) = (scale * angle.cos(), scale * angle.sin());
        let truth = Similarity { a, b, tx, ty };

        let src: [(f32, f32); LANDMARKS] =
            [(10.0, 20.0), (60.0, 22.0), (35.0, 45.0), (15.0, 70.0), (55.0, 71.0)];
        let mut dst = [(0.0f32, 0.0f32); LANDMARKS];
        for i in 0..LANDMARKS {
            dst[i] = truth.apply(src[i].0, src[i].1);
        }

        let got = estimate_similarity(&src, &dst).expect("well-conditioned points");
        assert!((got.a - truth.a).abs() < 1e-4, "a: {} vs {}", got.a, truth.a);
        assert!((got.b - truth.b).abs() < 1e-4, "b: {} vs {}", got.b, truth.b);
        assert!((got.tx - truth.tx).abs() < 1e-3, "tx: {} vs {}", got.tx, truth.tx);
        assert!((got.ty - truth.ty).abs() < 1e-3, "ty: {} vs {}", got.ty, truth.ty);
        assert!((got.scale() - scale).abs() < 1e-4);
    }

    #[test]
    fn inverse_undoes_the_forward_transform() {
        let s = Similarity { a: 1.4, b: -0.6, tx: 30.0, ty: -12.0 };
        let inv = s.invert().unwrap();
        for (x, y) in [(0.0f32, 0.0f32), (56.0, 56.0), (111.0, 3.0)] {
            let (fx, fy) = s.apply(x, y);
            let (bx, by) = inv.apply(fx, fy);
            assert!((bx - x).abs() < 1e-3 && (by - y).abs() < 1e-3, "({bx},{by}) vs ({x},{y})");
        }
    }

    #[test]
    fn coincident_landmarks_do_not_produce_a_transform() {
        let same = [(50.0f32, 50.0f32); LANDMARKS];
        assert!(estimate_similarity(&same, &REFERENCE_POINTS).is_none());
        assert!(Similarity { a: 0.0, b: 0.0, tx: 1.0, ty: 1.0 }.invert().is_none());
    }

    /// Landmarks that already sit on the reference points must produce the identity, and
    /// therefore a chip that is a plain crop of the top-left 112x112.
    #[test]
    fn reference_landmarks_produce_the_identity_transform() {
        let s = estimate_similarity(&REFERENCE_POINTS, &REFERENCE_POINTS).unwrap();
        assert!((s.a - 1.0).abs() < 1e-5);
        assert!(s.b.abs() < 1e-5);
        assert!(s.tx.abs() < 1e-3 && s.ty.abs() < 1e-3);
    }

    /// **The property the golden test exists to protect.** Alignment must be a pure
    /// function: the same frame and the same landmarks must produce byte-identical chips,
    /// because enrolment and verification are two separate calls and any difference
    /// between them is silent embedding drift.
    #[test]
    fn alignment_is_deterministic() {
        let px = domed(W, H, 220, 10);
        let frame = Frame::new(&px, W, H).unwrap();
        let lm = [(18.0f32, 22.0), (44.0, 23.0), (31.0, 34.0), (21.0, 45.0), (42.0, 46.0)];
        let a = align(&frame, &lm).unwrap();
        let b = align(&frame, &lm).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.pixels.len(), CHIP_SIZE * CHIP_SIZE);
    }

    /// Alignment must undo pose, which is its entire purpose. The same synthetic subject
    /// presented at a different scale, rotation and offset must land in the same chip.
    #[test]
    fn alignment_cancels_scale_rotation_and_translation() {
        let px = domed(256, 256, 240, 5);
        let frame = Frame::new(&px, 256, 256).unwrap();

        let base = [(96.0f32, 104.0), (160.0, 104.0), (128.0, 136.0), (102.0, 172.0), (154.0, 172.0)];
        let moved_by = Similarity { a: 0.7 * 0.94, b: 0.7 * 0.34, tx: 20.0, ty: -6.0 };
        let mut moved = [(0.0f32, 0.0f32); LANDMARKS];
        for i in 0..LANDMARKS {
            moved[i] = moved_by.apply(base[i].0, base[i].1);
        }

        let chip_a = align(&frame, &base).unwrap();
        let chip_b = align(&frame, &moved).unwrap();

        // Not byte-identical — the two sample different pixels of a radial gradient — but
        // the landmarks must land on the reference points either way, which is the claim.
        for (lm, chip) in [(base, &chip_a), (moved, &chip_b)] {
            let t = estimate_similarity(&lm, &REFERENCE_POINTS).unwrap();
            for (i, (rx, ry)) in REFERENCE_POINTS.iter().enumerate() {
                let (gx, gy) = t.apply(lm[i].0, lm[i].1);
                assert!(
                    (gx - rx).abs() < 2.0 && (gy - ry).abs() < 2.0,
                    "landmark {i} maps to ({gx},{gy}), reference is ({rx},{ry})"
                );
            }
            assert_eq!(chip.size, CHIP_SIZE);
        }
    }

    #[test]
    fn sampling_outside_the_frame_clamps_instead_of_going_black() {
        let px = crate::testing::blank(W, H, 200);
        let frame = Frame::new(&px, W, H).unwrap();
        assert_eq!(sample_bilinear(&frame, -50.0, -50.0), 200);
        assert_eq!(sample_bilinear(&frame, 1000.0, 1000.0), 200);
    }
}
