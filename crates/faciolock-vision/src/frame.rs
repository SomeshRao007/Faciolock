use crate::config::{Config, Tier};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn clamped(&self, width: usize, height: usize) -> Option<PixelRect> {
        let x0 = self.x.max(0.0).floor() as usize;
        let y0 = self.y.max(0.0).floor() as usize;
        let x1 = ((self.x + self.w).ceil() as isize).clamp(0, width as isize) as usize;
        let y1 = ((self.y + self.h).ceil() as isize).clamp(0, height as isize) as usize;
        if x1 <= x0 || y1 <= y0 || x0 >= width || y0 >= height {
            return None;
        }
        Some(PixelRect { x0, y0, w: x1 - x0, h: y1 - y0 })
    }

    pub fn area(&self) -> f32 {
        (self.w.max(0.0)) * (self.h.max(0.0))
    }

    pub fn iou(&self, other: &Rect) -> f32 {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 <= x0 || y1 <= y0 {
            return 0.0;
        }
        let inter = (x1 - x0) * (y1 - y0);
        let union = self.area() + other.area() - inter;
        if union <= 0.0 { 0.0 } else { inter / union }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}

impl PixelRect {
    pub fn tiled(&self, grid: usize) -> Option<PixelRect> {
        let w = (self.w / grid) * grid;
        let h = (self.h / grid) * grid;
        if w == 0 || h == 0 {
            return None;
        }
        Some(PixelRect { x0: self.x0, y0: self.y0, w, h })
    }
}

#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}

impl<'a> Frame<'a> {
    pub fn new(pixels: &'a [u8], width: usize, height: usize) -> Option<Frame<'a>> {
        if width == 0 || height == 0 || pixels.len() != width * height {
            return None;
        }
        Some(Frame { pixels, width, height })
    }

    pub fn mean(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.pixels.iter().map(|&p| u64::from(p)).sum();
        sum as f64 / self.pixels.len() as f64
    }

    pub fn mean_in(&self, r: &PixelRect) -> f64 {
        let mut sum = 0u64;
        for y in r.y0..r.y0 + r.h {
            let start = y * self.width + r.x0;
            sum += self.pixels[start..start + r.w].iter().map(|&p| u64::from(p)).sum::<u64>();
        }
        sum as f64 / (r.w * r.h) as f64
    }

    pub fn cell_means(&self, r: &PixelRect, grid: usize) -> Vec<f64> {
        let cw = r.w / grid;
        let ch = r.h / grid;
        let px = (cw * ch) as f64;
        let mut out = Vec::with_capacity(grid * grid);
        for gy in 0..grid {
            for gx in 0..grid {
                let mut sum = 0u64;
                for y in r.y0 + gy * ch..r.y0 + (gy + 1) * ch {
                    let start = y * self.width + r.x0 + gx * cw;
                    sum += self.pixels[start..start + cw].iter().map(|&p| u64::from(p)).sum::<u64>();
                }
                out.push(sum as f64 / px);
            }
        }
        out
    }

    pub fn mean_abs_diff_in(&self, other: &Frame<'_>, r: &PixelRect) -> f64 {
        let mut sum = 0u64;
        for y in r.y0..r.y0 + r.h {
            let a = y * self.width + r.x0;
            let b = y * other.width + r.x0;
            for i in 0..r.w {
                sum += u64::from(self.pixels[a + i].abs_diff(other.pixels[b + i]));
            }
        }
        sum as f64 / (r.w * r.h) as f64
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    On,
    Off,
}

#[derive(Clone, Copy)]
pub struct Pair<'a> {
    pub on: Frame<'a>,
    pub off: Frame<'a>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observed {
    Strobing,
    Constant,
    Dark,
    Irregular,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BurstError {
    AmbientTooBright { floor: f32 },
    TooFewFrames { got: usize },
    Malformed,
    TierMismatch { configured: Tier, observed: Observed },
    TooFewPairs { got: usize, need: usize },
}

pub struct Burst<'a> {
    pairs: Vec<Pair<'a>>,
    observed: Observed,
    width: usize,
    height: usize,
}

impl<'a> Burst<'a> {
    pub fn from_frames(frames: &[Frame<'a>], cfg: &Config) -> Result<Burst<'a>, BurstError> {
        let (w, h) = match frames.first() {
            None => return Err(BurstError::TooFewFrames { got: 0 }),
            Some(f) => (f.width, f.height),
        };
        if frames.iter().any(|f| f.width != w || f.height != h) {
            return Err(BurstError::Malformed);
        }

        let usable = &frames[cfg.agc_settle_frames.min(frames.len())..];
        if usable.len() < 2 {
            return Err(BurstError::TooFewFrames { got: usable.len() });
        }

        let means: Vec<f64> = usable.iter().map(|f| f.mean()).collect();

        let floor = means.iter().cloned().fold(f64::INFINITY, f64::min);
        if floor > cfg.ambient_floor_max {
            return Err(BurstError::AmbientTooBright { floor: floor as f32 });
        }

        let (phases, observed) = classify(&means);

        let conforms = match cfg.tier {
            Tier::IrStrobe => observed == Observed::Strobing,
            Tier::IrConstant => observed == Observed::Constant,
            Tier::Rgb => observed != Observed::Dark,
        };
        if !conforms {
            return Err(BurstError::TierMismatch { configured: cfg.tier, observed });
        }

        let pairs = build_pairs(usable, &phases);
        if pairs.len() < cfg.min_pairs {
            return Err(BurstError::TooFewPairs { got: pairs.len(), need: cfg.min_pairs });
        }

        Ok(Burst { pairs, observed, width: w, height: h })
    }

    pub fn from_planar(
        bytes: &'a [u8],
        width: usize,
        height: usize,
        cfg: &Config,
    ) -> Result<Burst<'a>, BurstError> {
        let stride = width.checked_mul(height).filter(|&s| s > 0).ok_or(BurstError::Malformed)?;
        if bytes.len() < stride || !bytes.len().is_multiple_of(stride) {
            return Err(BurstError::Malformed);
        }
        let frames: Vec<Frame<'a>> = bytes
            .chunks_exact(stride)
            .filter_map(|c| Frame::new(c, width, height))
            .collect();
        Burst::from_frames(&frames, cfg)
    }

    pub fn pairs(&self) -> &[Pair<'a>] {
        &self.pairs
    }
    pub fn observed(&self) -> Observed {
        self.observed
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
}

const STROBE_SNR_MIN: f64 = 10.0;

const STROBE_REGULARITY_MIN: f64 = 0.9;

pub fn classify(means: &[f64]) -> (Vec<Phase>, Observed) {
    if means.len() < 3 {
        return (means.iter().map(|_| Phase::On).collect(), Observed::Irregular);
    }

    let hi = means.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let steps: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();

    let amplitude = median_abs(&steps);
    let same_phase = median_abs(&means.windows(3).map(|w| w[2] - w[0]).collect::<Vec<_>>());
    let snr = match (amplitude > 0.0, same_phase > 0.0) {
        (true, false) => f64::INFINITY,
        (true, true) => amplitude / same_phase,
        (false, _) => 0.0,
    };

    let reversals = steps.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    let regularity = reversals as f64 / steps.len().saturating_sub(1).max(1) as f64;

    if snr < STROBE_SNR_MIN || regularity < STROBE_REGULARITY_MIN {
        let observed = if hi < 1.0 {
            Observed::Dark
        } else if snr < STROBE_SNR_MIN {
            Observed::Constant
        } else {
            Observed::Irregular
        };
        return (means.iter().map(|_| Phase::On).collect(), observed);
    }

    let phases: Vec<Phase> = (0..means.len())
        .map(|i| {
            let reference = match (i.checked_sub(1), means.get(i + 1)) {
                (Some(p), Some(&n)) => (means[p] + n) / 2.0,
                (Some(p), None) => means[p],
                (None, Some(&n)) => n,
                (None, None) => return Phase::On,
            };
            if means[i] > reference { Phase::On } else { Phase::Off }
        })
        .collect();

    (phases, Observed::Strobing)
}

fn median_abs(v: &[f64]) -> f64 {
    let mut s: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

fn build_pairs<'a>(frames: &[Frame<'a>], phases: &[Phase]) -> Vec<Pair<'a>> {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < phases.len() {
        let (on_i, off_i) = match (phases[i], phases[i + 1]) {
            (Phase::On, Phase::Off) => (i, i + 1),
            (Phase::Off, Phase::On) => (i + 1, i),
            _ => {
                i += 1;
                continue;
            }
        };
        pairs.push(Pair { on: frames[on_i], off: frames[off_i] });
        i += 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{blank, domed, flat, strobe, H, W};

    fn cfg() -> Config {
        Config { agc_settle_frames: 0, ..Config::default() }
    }

    #[test]
    fn mean_is_the_arithmetic_mean() {
        let px = blank(W, H, 40);
        let f = Frame::new(&px, W, H).unwrap();
        assert!((f.mean() - 40.0).abs() < 1e-9);
        let r = PixelRect { x0: 8, y0: 8, w: 24, h: 24 };
        assert!((f.mean_in(&r) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn frame_rejects_a_buffer_of_the_wrong_length() {
        assert!(Frame::new(&[0u8; 10], W, H).is_none());
        assert!(Frame::new(&[], 0, 0).is_none());
    }

    #[test]
    fn tiled_rect_divides_evenly_into_cells() {
        let r = PixelRect { x0: 0, y0: 0, w: 40, h: 40 }.tiled(6).unwrap();
        assert_eq!(r.w % 6, 0);
        assert_eq!(r.h % 6, 0);
        let px = blank(W, H, 10);
        let f = Frame::new(&px, W, H).unwrap();
        assert_eq!(f.cell_means(&r, 6).len(), 36);
    }

    #[test]
    fn a_rect_entirely_outside_the_frame_clamps_to_nothing() {
        let r = Rect { x: -50.0, y: -50.0, w: 10.0, h: 10.0 };
        assert!(r.clamped(W, H).is_none());
        let r = Rect { x: 5.0, y: 5.0, w: 1000.0, h: 1000.0 };
        assert_eq!(r.clamped(W, H).unwrap().w, W - 5);
    }

    #[test]
    fn iou_is_one_for_identical_boxes_and_zero_for_disjoint_ones() {
        let a = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let b = Rect { x: 20.0, y: 20.0, w: 10.0, h: 10.0 };
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
        assert_eq!(a.iou(&b), 0.0);
    }

    #[test]
    fn classify_detects_a_strobe() {
        let means: Vec<f64> = (0..30).map(|i| if i % 2 == 0 { 60.0 } else { 0.004 }).collect();
        let (phases, observed) = classify(&means);
        assert_eq!(observed, Observed::Strobing);
        assert_eq!(phases[0], Phase::On);
        assert_eq!(phases[1], Phase::Off);
    }

    #[test]
    fn classify_detects_constant_illumination() {
        let means: Vec<f64> = (0..30).map(|i| 55.0 + (i % 3) as f64).collect();
        assert_eq!(classify(&means).1, Observed::Constant);
    }

    #[test]
    fn classify_detects_a_dark_emitter() {
        let means: Vec<f64> = (0..30).map(|i| 0.004 + (i % 3) as f64 * 0.0001).collect();
        assert_eq!(classify(&means).1, Observed::Dark);
    }

    fn constant_capture(level: u8) -> Vec<u8> {
        blank(W, H, level)
    }

    #[test]
    fn constant_illumination_is_rejected_not_downgraded() {
        let px = constant_capture(60);
        let frames: Vec<Frame<'_>> = (0..20).map(|_| Frame::new(&px, W, H).unwrap()).collect();
        match Burst::from_frames(&frames, &cfg()) {
            Err(BurstError::AmbientTooBright { floor }) => {
                assert!((floor - 60.0).abs() < 0.5, "floor {floor} should be the frame mean");
            }
            other => panic!("expected the ambient gate, got {:?}", other.map(|_| "a burst")),
        }
    }

    #[test]
    fn no_constant_capture_ever_yields_a_burst() {
        for level in [0u8, 1, 5, 30, 60, 120, 200, 255] {
            let px = constant_capture(level);
            let frames: Vec<Frame<'_>> = (0..20).map(|_| Frame::new(&px, W, H).unwrap()).collect();
            assert!(
                Burst::from_frames(&frames, &cfg()).is_err(),
                "constant illumination at {level} produced a burst"
            );
        }
    }

    #[test]
    fn the_gate_measures_the_floor_and_not_the_peak() {
        let on = domed(W, H, 200, 150);
        let off = blank(W, H, 0);
        let bytes = strobe(&on, &off, 4);
        assert!(
            Burst::from_planar(&bytes, W, H, &cfg()).is_ok(),
            "a bright emitter over a dark room must not trip the ambient gate"
        );
    }

    #[test]
    fn a_burst_with_too_few_pairs_is_an_error_not_an_empty_pass() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let frames = vec![
            Frame::new(&on, W, H).unwrap(),
            Frame::new(&off, W, H).unwrap(),
            Frame::new(&on, W, H).unwrap(),
        ];
        assert!(matches!(
            Burst::from_frames(&frames, &cfg()),
            Err(BurstError::TooFewPairs { got: 1, need: 2 })
        ));
    }

    #[test]
    fn a_capture_too_short_to_measure_is_not_a_strobe() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let frames = vec![Frame::new(&on, W, H).unwrap(), Frame::new(&off, W, H).unwrap()];
        assert!(matches!(
            Burst::from_frames(&frames, &cfg()),
            Err(BurstError::TierMismatch { observed: Observed::Irregular, .. })
        ));
        assert_eq!(classify(&[60.0, 0.0]).1, Observed::Irregular);
    }

    #[test]
    fn pairs_never_reuse_a_frame() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let mut frames = Vec::new();
        for i in 0..20 {
            let px: &[u8] = if i % 2 == 0 { &on } else { &off };
            frames.push(Frame::new(px, W, H).unwrap());
        }
        let burst = Burst::from_frames(&frames, &cfg()).unwrap();
        assert_eq!(burst.observed(), Observed::Strobing);
        assert_eq!(burst.pairs().len(), 10);
    }

    #[test]
    fn planar_input_must_be_a_whole_number_of_frames() {
        let bytes = vec![0u8; W * H + 7];
        assert!(matches!(
            Burst::from_planar(&bytes, W, H, &cfg()),
            Err(BurstError::Malformed)
        ));
        assert!(matches!(
            Burst::from_planar(&[0u8; 10], W, H, &cfg()),
            Err(BurstError::Malformed)
        ));
    }

    #[test]
    fn mean_abs_diff_sees_a_change_the_mean_alone_cannot() {
        let a = flat(W, H, 60);
        let mut b = vec![0u8; W * H];
        for (i, p) in b.iter_mut().enumerate() {
            *p = if i % 2 == 0 { 0 } else { 120 };
        }
        let fa = Frame::new(&a, W, H).unwrap();
        let fb = Frame::new(&b, W, H).unwrap();
        let r = PixelRect { x0: 0, y0: 0, w: W, h: H };
        assert_eq!(fa.mean(), fb.mean(), "means are equal by construction");
        assert!(fa.mean_abs_diff_in(&fb, &r) > 50.0, "per-pixel difference is not");
    }
}
