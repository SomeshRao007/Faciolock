//! Phase 0 strobe-liveness probe.
//!
//! Reads a raw Y800 burst recorded by `tools/record.sh` and reports the four
//! quantities the liveness design in `wiki/05-vision-pipeline.md` rests on:
//!
//!   * ON/OFF classification by mean luma      -- does this hardware self-strobe?
//!   * D = mean(ON) - mean(OFF) over the ROI   -- response test
//!   * mean(OFF)/mean(ON)                      -- dark-frame integrity, as a RATIO
//!   * spatial falloff of D across a cell grid -- the print-vs-face signal
//!
//! Everything is a ratio or a shape. Nothing is thresholded on absolute brightness:
//! the OFF floor moved 10x between two runs on the same machine and the ON series
//! does not decay monotonically. See wiki/10 Probe 3b.
//!
//! This is a measurement tool, not a classifier. It prints numbers so thresholds can
//! be chosen from data. It deliberately does not emit a live/spoof verdict -- that
//! decision belongs in `faciolock-vision` once there is a corpus to calibrate against.
//!
//! Usage:  probe <file.gray> [width] [height]

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Frames to drop from the head of a capture. The AGC needs a few frames to converge
/// and the first ones are garbage -- 22.7, 23.4, 14.3, 31.4 on the reference machine
/// before it settles into a clean alternation. wiki/10 Probe 3.
const AGC_SETTLE_FRAMES: usize = 5;

/// Cell grid used for the falloff test. 6x6 over the ROI gives 36 samples: enough
/// for the shape of the falloff to show up, coarse enough that each cell still
/// averages over ~4800 pixels and is not dominated by sensor noise.
const GRID: usize = 6;

/// Fraction of the frame treated as the region of interest, centred.
///
// ponytail: centre crop instead of a real face box. At laptop distance the face
// fills most of the frame, which is enough to compare live-vs-print falloff. Swap
// for the YuNet detection box in Phase 1 -- at that point the ROI tracks the face
// instead of assuming it is centred, and off-centre captures stop being noise.
const ROI_FRACTION: f64 = 0.5;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <file.gray> [width] [height]", args[0]);
        eprintln!("       defaults to 640x360 (the reference IR camera)");
        std::process::exit(2);
    }

    let path = &args[1];
    let width = parse_dim(args.get(2), 640, "width");
    let height = parse_dim(args.get(3), 360, "height");

    // A capture with no sidecar is a capture nobody can interpret: no record of what
    // was in front of the camera, so it can neither be trusted as a positive nor as a
    // negative. Warn rather than refuse -- the pixels are still readable, and the
    // operator may know what it is. But it must not join the corpus silently.
    if let Some(stem) = path.strip_suffix(".gray") {
        let sidecar = format!("{stem}.json");
        if !std::path::Path::new(&sidecar).exists() {
            eprintln!("warning: no sidecar at {sidecar} -- this capture is undocumented");
        }
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    match Capture::new(bytes, width, height) {
        Ok(cap) => print!("{}", cap.report()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_dim(arg: Option<&String>, default: usize, name: &str) -> usize {
    match arg {
        None => default,
        Some(s) => match s.parse::<usize>() {
            Ok(v) if v > 0 => v,
            _ => {
                eprintln!("error: {name} must be a positive integer, got '{s}'");
                std::process::exit(2);
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Roi {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

impl Roi {
    fn centred(width: usize, height: usize, fraction: f64) -> Roi {
        // Round the ROI down to a whole number of grid cells so every cell holds the
        // same pixel count. Unequal cells would put a systematic bias in the falloff
        // variance, which is the one number this tool exists to measure.
        let rw = ((width as f64 * fraction) as usize / GRID) * GRID;
        let rh = ((height as f64 * fraction) as usize / GRID) * GRID;
        Roi {
            x0: (width - rw) / 2,
            y0: (height - rh) / 2,
            w: rw.max(GRID),
            h: rh.max(GRID),
        }
    }
}

// ---------------------------------------------------------------------------
// Frame arithmetic
// ---------------------------------------------------------------------------

/// Mean luma over the whole frame.
fn mean_frame(frame: &[u8]) -> f64 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: u64 = frame.iter().map(|&p| u64::from(p)).sum();
    sum as f64 / frame.len() as f64
}

/// Mean luma over the ROI.
fn mean_roi(frame: &[u8], width: usize, roi: &Roi) -> f64 {
    let mut sum = 0u64;
    for y in roi.y0..roi.y0 + roi.h {
        let start = y * width + roi.x0;
        sum += frame[start..start + roi.w]
            .iter()
            .map(|&p| u64::from(p))
            .sum::<u64>();
    }
    sum as f64 / (roi.w * roi.h) as f64
}

/// Mean luma of each cell of a GRID x GRID tiling of the ROI, row-major.
fn cell_means(frame: &[u8], width: usize, roi: &Roi) -> Vec<f64> {
    let cw = roi.w / GRID;
    let ch = roi.h / GRID;
    let px = (cw * ch) as f64;
    let mut out = Vec::with_capacity(GRID * GRID);

    for gy in 0..GRID {
        for gx in 0..GRID {
            let mut sum = 0u64;
            for y in roi.y0 + gy * ch..roi.y0 + (gy + 1) * ch {
                let start = y * width + roi.x0 + gx * cw;
                sum += frame[start..start + cw]
                    .iter()
                    .map(|&p| u64::from(p))
                    .sum::<u64>();
            }
            out.push(sum as f64 / px);
        }
    }
    out
}

/// Coefficient of variation of the per-cell response.
///
/// This is the falloff metric. A live face is a curved, three-dimensional diffuse
/// reflector: cells covering the nose and forehead brighten far more than cells on
/// the cheeks and jaw, so the spread of D across cells is large relative to its mean.
/// A flat print reflects near-uniformly, so the spread collapses.
///
/// Expressed as std/mean rather than raw std so it stays comparable across captures
/// at different distances and different AGC states -- the same reason everything
/// else here is a ratio.
fn falloff_cv(d_cells: &[f64]) -> f64 {
    let m = mean(d_cells);
    if m.abs() < f64::EPSILON {
        return 0.0;
    }
    let var = d_cells.iter().map(|v| (v - m).powi(2)).sum::<f64>() / d_cells.len() as f64;
    var.sqrt() / m
}

/// Ratio of response in the centre cells to response in the border ring.
///
/// A second, independent read on the same physics as `falloff_cv`, and one that has a
/// direction: on a face the centre (nose, brow -- nearest the emitter) outshines the
/// periphery, so this sits above 1. On a flat surface held square to the camera it
/// sits near 1. Reported alongside CV because a crumpled or tilted print could raise
/// CV without producing a centre-bright pattern.
fn centre_edge_ratio(d_cells: &[f64]) -> f64 {
    let mut centre = Vec::new();
    let mut edge = Vec::new();

    for gy in 0..GRID {
        for gx in 0..GRID {
            let v = d_cells[gy * GRID + gx];
            if gx == 0 || gy == 0 || gx == GRID - 1 || gy == GRID - 1 {
                edge.push(v);
            } else if (GRID / 2 - 1..=GRID / 2).contains(&gx)
                && (GRID / 2 - 1..=GRID / 2).contains(&gy)
            {
                centre.push(v);
            }
        }
    }

    let e = mean(&edge);
    if e.abs() < f64::EPSILON {
        return f64::INFINITY;
    }
    mean(&centre) / e
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Percentile by nearest rank. `xs` must already be sorted ascending.
fn percentile(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let idx = (((xs.len() - 1) as f64) * p).round() as usize;
    xs[idx]
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

#[derive(PartialEq, Clone, Copy, Debug)]
enum Phase {
    On,
    Off,
}

/// What the alternation pattern says about this hardware. Maps onto the capability
/// tiers in wiki/05 -- the point of classifying by observation is that it needs no
/// writes to the device.
#[derive(PartialEq, Debug)]
enum Tier {
    /// Clean ON/OFF alternation. Full liveness available.
    IrStrobe,
    /// Emitter appears to be on continuously. Passive checks only.
    IrConstant,
    /// Every frame dark. Emitter is off -- needs one-time setup.
    EmitterDark,
    /// Frames vary but not in a strobe pattern.
    Irregular,
}

impl Tier {
    fn describe(&self) -> &'static str {
        match self {
            Tier::IrStrobe => "ir-strobe    -- full liveness: response, falloff, dark-frame integrity",
            Tier::IrConstant => "ir-constant  -- passive checks only; no strobe difference available",
            Tier::EmitterDark => "emitter dark -- no illumination; needs linux-enable-ir-emitter (one-time)",
            Tier::Irregular => "irregular    -- frames vary but not as a strobe; investigate before trusting",
        }
    }
}

struct Pair {
    index: usize,
    on_mean: f64,
    off_mean: f64,
    /// Response: mean(ON) - mean(OFF) over the ROI.
    d: f64,
    /// mean(ON)/mean(OFF). The quantity that stayed stable across sessions.
    ratio: f64,
    /// mean(OFF)/mean(ON). Dark-frame integrity -- high means the scene is being
    /// flooded with external IR and the difference test is being washed out.
    dark_ratio: f64,
    falloff_cv: f64,
    centre_edge: f64,
    /// Per-cell response, row-major over the GRID x GRID tiling. Retained so the
    /// report can show *where* the response is, not just how uneven it is -- a
    /// summary statistic cannot tell you whether the ROI is even pointed at a face.
    d_cells: Vec<f64>,
}

struct Capture {
    width: usize,
    height: usize,
    frame_count: usize,
    frame_means: Vec<f64>,
    phases: Vec<Phase>,
    tier: Tier,
    roi: Roi,
    pairs: Vec<Pair>,
    alternation: Option<Alternation>,
}

impl Capture {
    fn new(bytes: Vec<u8>, width: usize, height: usize) -> Result<Capture, String> {
        let frame_bytes = width * height;
        if bytes.len() < frame_bytes {
            return Err(format!(
                "file holds {} bytes, less than one {width}x{height} frame ({frame_bytes} bytes)",
                bytes.len()
            ));
        }
        if !bytes.len().is_multiple_of(frame_bytes) {
            return Err(format!(
                "file holds {} bytes, not a whole number of {width}x{height} frames \
                 ({frame_bytes} bytes each) -- wrong geometry?",
                bytes.len()
            ));
        }

        let frame_count = bytes.len() / frame_bytes;
        let frames: Vec<&[u8]> = bytes.chunks_exact(frame_bytes).collect();
        let frame_means: Vec<f64> = frames.iter().map(|f| mean_frame(f)).collect();

        let usable = AGC_SETTLE_FRAMES.min(frame_count);
        let (phases, tier) = classify(&frame_means[usable..]);

        let roi = Roi::centred(width, height, ROI_FRACTION);
        let pairs = build_pairs(&frames[usable..], &phases, width, &roi);

        // Deliberately computed from the same post-AGC means the ratio test saw, so the
        // two views are answering the same question about the same data and any
        // disagreement between them is a real disagreement.
        let alternation = alternation(&frame_means[usable..]);

        Ok(Capture {
            width,
            height,
            frame_count,
            frame_means,
            phases,
            tier,
            roi,
            pairs,
            alternation,
        })
    }

    fn report(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{}x{}  {} frames  ({} discarded for AGC settling)",
            self.width,
            self.height,
            self.frame_count,
            AGC_SETTLE_FRAMES.min(self.frame_count)
        );
        let _ = writeln!(
            s,
            "ROI {}x{} at ({},{})  --  {GRID}x{GRID} cells of {}x{} px",
            self.roi.w,
            self.roi.h,
            self.roi.x0,
            self.roi.y0,
            self.roi.w / GRID,
            self.roi.h / GRID
        );
        let _ = writeln!(s, "\ntier: {}", self.tier.describe());

        let _ = writeln!(s, "\n-- frame means (whole frame) --");
        for (i, m) in self.frame_means.iter().enumerate() {
            let tag = if i < AGC_SETTLE_FRAMES.min(self.frame_count) {
                "agc"
            } else {
                match self.phases.get(i - AGC_SETTLE_FRAMES.min(self.frame_count)) {
                    Some(Phase::On) => "ON ",
                    Some(Phase::Off) => "off",
                    None => "   ",
                }
            };
            let _ = write!(s, "{i:>3} {tag} {m:>10.5}   ");
            if i % 4 == 3 {
                let _ = writeln!(s);
            }
        }
        if !self.frame_means.len().is_multiple_of(4) {
            let _ = writeln!(s);
        }

        if let Some(a) = self.alternation {
            let _ = writeln!(s, "\n-- alternation, difference domain (whole frame) --");
            let _ = writeln!(s, "{:>18}  {:>12.4}", "ambient pedestal", a.pedestal);
            let _ = writeln!(s, "{:>18}  {:>12.4}", "amplitude", a.amplitude);
            let _ = writeln!(s, "{:>18}  {:>12.4}", "same-phase noise", a.same_phase);
            let _ = writeln!(s, "{:>18}  {:>12.1}", "SNR", a.snr);
            let _ = writeln!(s, "{:>18}  {:>12.3}", "sign regularity", a.sign_regularity);
            let _ = writeln!(s, "{:>18}  {:>12.4}", "modulation", a.modulation);
        }

        if self.pairs.is_empty() {
            let _ = writeln!(
                s,
                "\nNo ON/OFF pairs found: the *ratio* test above did not see a strobe, so\n\
                 nothing was split into phases. Read the difference-domain block instead --\n\
                 a high SNR with sign regularity near 1.0 means the emitter is alternating\n\
                 perfectly well and only the ratio statistic failed to notice. wiki/11 B18."
            );
            return s;
        }

        let _ = writeln!(
            s,
            "\n-- per pair (ROI) --\n\
             {:>4}  {:>9}  {:>9}  {:>9}  {:>11}  {:>10}  {:>9}  {:>9}",
            "pair", "mean(ON)", "mean(OFF)", "D", "ON/OFF", "OFF/ON", "falloffCV", "ctr/edge"
        );
        for p in &self.pairs {
            let _ = writeln!(
                s,
                "{:>4}  {:>9.4}  {:>9.5}  {:>9.4}  {:>11.0}  {:>10.2e}  {:>9.4}  {:>9.4}",
                p.index, p.on_mean, p.off_mean, p.d, p.ratio, p.dark_ratio, p.falloff_cv, p.centre_edge
            );
        }

        let _ = writeln!(s, "\n-- summary over {} pairs --", self.pairs.len());
        let _ = writeln!(s, "{:>12}  {:>12}  {:>12}  {:>12}", "", "p05", "p50", "p95");
        for (name, vals) in self.metric_columns() {
            let mut v = vals;
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let _ = writeln!(
                s,
                "{name:>12}  {:>12.4}  {:>12.4}  {:>12.4}",
                percentile(&v, 0.05),
                percentile(&v, 0.50),
                percentile(&v, 0.95)
            );
        }

        let _ = write!(s, "{}", self.cell_map());

        let _ = writeln!(
            s,
            "\nNo verdict is emitted by design. These are the inputs to a threshold,\n\
             and the threshold needs a spoof sample to sit against. See wiki/09."
        );
        s
    }

    /// Mean response per cell, as a percentage of the strongest cell.
    ///
    /// The summary statistics say how uneven the response is. They cannot say whether
    /// the unevenness is a face, or the ROI hanging off the edge of one. This map can:
    /// a centred face is a bright blob in the middle, and anything else is not.
    fn cell_map(&self) -> String {
        let mut s = String::new();
        if self.pairs.is_empty() {
            return s;
        }

        let mut avg = vec![0.0; GRID * GRID];
        for p in &self.pairs {
            for (a, v) in avg.iter_mut().zip(&p.d_cells) {
                *a += v;
            }
        }
        for a in &mut avg {
            *a /= self.pairs.len() as f64;
        }

        let peak = avg.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let _ = writeln!(
            s,
            "\n-- mean response per cell, % of peak ({peak:.1}) --\n\
             (a centred face reads as a bright centre; a flat or empty scene reads even)"
        );
        for gy in 0..GRID {
            let _ = write!(s, "  ");
            for gx in 0..GRID {
                let pct = if peak.abs() < f64::EPSILON {
                    0.0
                } else {
                    avg[gy * GRID + gx] / peak * 100.0
                };
                let _ = write!(s, "{pct:>6.0}");
            }
            let _ = writeln!(s);
        }
        s
    }

    fn metric_columns(&self) -> BTreeMap<&'static str, Vec<f64>> {
        let mut m = BTreeMap::new();
        m.insert("D", self.pairs.iter().map(|p| p.d).collect());
        m.insert("ON/OFF", self.pairs.iter().map(|p| p.ratio).collect());
        m.insert("OFF/ON", self.pairs.iter().map(|p| p.dark_ratio).collect());
        m.insert("falloffCV", self.pairs.iter().map(|p| p.falloff_cv).collect());
        m.insert("ctr/edge", self.pairs.iter().map(|p| p.centre_edge).collect());
        m
    }
}

/// Split frames into ON and OFF by mean luma, and judge what pattern they form.
///
/// The split point is the midpoint between the dimmest and brightest frame. On
/// strobing hardware the two populations are separated by four to five orders of
/// magnitude, so where exactly the cut falls between them does not matter. What does
/// matter is recognising when there is no separation at all, which is the
/// `ir-constant` and `emitter-dark` case -- hence the spread check first.
fn classify(means: &[f64]) -> (Vec<Phase>, Tier) {
    if means.is_empty() {
        return (Vec::new(), Tier::Irregular);
    }

    let lo = means.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = means.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Require the bright population to be at least 10x the dark one before calling
    // it a strobe. Anything less is AGC wobble on a constantly-lit sensor, not an
    // emitter being switched.
    let separated = hi > lo * 10.0 && hi > 1.0;

    if !separated {
        let tier = if hi < 1.0 {
            Tier::EmitterDark
        } else {
            Tier::IrConstant
        };
        return (means.iter().map(|_| Phase::On).collect(), tier);
    }

    let split = (lo + hi) / 2.0;
    let phases: Vec<Phase> = means
        .iter()
        .map(|&m| if m > split { Phase::On } else { Phase::Off })
        .collect();

    // A strobe alternates every frame. Count how often it actually does.
    let flips = phases.windows(2).filter(|w| w[0] != w[1]).count();
    let tier = if flips >= phases.len().saturating_sub(1) * 9 / 10 {
        Tier::IrStrobe
    } else {
        Tier::Irregular
    };

    (phases, tier)
}

/// The strobe seen as a *difference* rather than a *ratio*.
///
/// Added 2026-08-06, because the ratio view failed. `classify` above asks whether the
/// brightest frame is 10x the dimmest, which is 15,000:1 at night and 1.15:1 in daylight
/// on the same hardware with the same emitter firing every single frame. Ambient NIR adds
/// a pedestal to both phases and a ratio saturates against a pedestal; a difference does
/// not. wiki/11 B18.
///
/// Nothing here is thresholded. It reports the quantities a threshold would have to be
/// chosen from, which is this tool's whole job.
#[derive(Clone, Copy)]
struct Alternation {
    /// Median |m[i+1] - m[i]|. The emitter's contribution to the frame mean.
    amplitude: f64,
    /// Median |m[i+2] - m[i]| -- consecutive frames of the *same* phase, so the emitter
    /// contributes nothing and what is left is AGC drift, scene motion and read noise.
    /// This is the noise floor the emitter has to beat, measured on the same capture
    /// rather than assumed.
    same_phase: f64,
    /// amplitude / same_phase. Dimensionless, and unlike the ratio it does not care what
    /// the ambient pedestal is.
    snr: f64,
    /// Fraction of steps where the difference reverses sign. A strobe alternates every
    /// frame, so a real one sits at ~1.0. Distinguishes an emitter from a scene that
    /// merely got brighter.
    sign_regularity: f64,
    /// Dimmest frame mean: the ambient floor the emitter is riding on.
    pedestal: f64,
    /// amplitude / mean. What fraction of the signal the emitter accounts for -- the
    /// quantity that collapsed in daylight even though `amplitude` did not.
    modulation: f64,
}

fn alternation(means: &[f64]) -> Option<Alternation> {
    if means.len() < 3 {
        return None;
    }

    let steps: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();
    let mut mags: Vec<f64> = steps.iter().map(|d| d.abs()).collect();

    // Same-phase pairs skip a frame, so the emitter state is identical at both ends.
    let mut same: Vec<f64> = means.windows(3).map(|w| (w[2] - w[0]).abs()).collect();

    mags.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    same.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let amplitude = percentile(&mags, 0.50);
    let same_phase = percentile(&same, 0.50);
    let flips = steps.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    let pedestal = means.iter().cloned().fold(f64::INFINITY, f64::min);
    let overall = mean(means);

    Some(Alternation {
        amplitude,
        same_phase,
        // A same-phase floor of exactly zero is a real reading on this hardware at night:
        // two true dark frames are identical. Report it as unbounded rather than dividing
        // by zero, which is honest -- the emitter beat the noise by more than the capture
        // can resolve.
        snr: if same_phase > 0.0 { amplitude / same_phase } else { f64::INFINITY },
        sign_regularity: flips as f64 / steps.len().saturating_sub(1).max(1) as f64,
        pedestal,
        modulation: if overall.abs() > f64::EPSILON { amplitude / overall } else { 0.0 },
    })
}

/// Pair each ON frame with an adjacent OFF frame and measure the pair.
///
/// Adjacency matters: pairing an ON frame with a distant OFF frame would let AGC
/// drift and scene motion contaminate D. Neighbours are ~33 ms apart, which is short
/// enough that the only thing that changed is the emitter.
fn build_pairs(frames: &[&[u8]], phases: &[Phase], width: usize, roi: &Roi) -> Vec<Pair> {
    let mut pairs = Vec::new();

    for i in 0..phases.len().saturating_sub(1) {
        let (on_i, off_i) = match (phases[i], phases[i + 1]) {
            (Phase::On, Phase::Off) => (i, i + 1),
            (Phase::Off, Phase::On) => (i + 1, i),
            _ => continue,
        };
        let on = frames[on_i];
        let off = frames[off_i];

        let on_mean = mean_roi(on, width, roi);
        let off_mean = mean_roi(off, width, roi);

        let on_cells = cell_means(on, width, roi);
        let off_cells = cell_means(off, width, roi);
        let d_cells: Vec<f64> = on_cells
            .iter()
            .zip(&off_cells)
            .map(|(a, b)| a - b)
            .collect();

        pairs.push(Pair {
            index: pairs.len(),
            on_mean,
            off_mean,
            d: on_mean - off_mean,
            ratio: if off_mean.abs() < f64::EPSILON {
                f64::INFINITY
            } else {
                on_mean / off_mean
            },
            dark_ratio: if on_mean.abs() < f64::EPSILON {
                f64::INFINITY
            } else {
                off_mean / on_mean
            },
            falloff_cv: falloff_cv(&d_cells),
            centre_edge: centre_edge_ratio(&d_cells),
            d_cells,
        });
    }

    // Keep every other pair so no frame is used twice.
    pairs
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .enumerate()
        .map(|(new_i, (_, mut p))| {
            p.index = new_i;
            p
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 64;
    const H: usize = 64;

    fn blank(v: u8) -> Vec<u8> {
        vec![v; W * H]
    }

    /// A flat reflector: uniform response everywhere, like a printed photo.
    fn flat(v: u8) -> Vec<u8> {
        blank(v)
    }

    /// A domed reflector: brightest at the centre, falling off radially, like a face
    /// lit by an emitter beside the lens.
    fn domed(peak: u8, floor: u8) -> Vec<u8> {
        let mut f = vec![0u8; W * H];
        let (cx, cy) = (W as f64 / 2.0, H as f64 / 2.0);
        let max_r = (cx * cx + cy * cy).sqrt();
        for y in 0..H {
            for x in 0..W {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let t = (dx * dx + dy * dy).sqrt() / max_r; // 0 at centre, 1 at corner
                let v = peak as f64 - t * (peak - floor) as f64;
                f[y * W + x] = v.clamp(0.0, 255.0) as u8;
            }
        }
        f
    }

    fn roi() -> Roi {
        Roi::centred(W, H, ROI_FRACTION)
    }

    #[test]
    fn mean_is_arithmetic_mean() {
        assert!((mean_frame(&blank(40)) - 40.0).abs() < 1e-9);
        assert!((mean_roi(&blank(40), W, &roi()) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn roi_divides_evenly_into_cells() {
        let r = roi();
        assert_eq!(r.w % GRID, 0, "ROI width must tile exactly into {GRID} cells");
        assert_eq!(r.h % GRID, 0, "ROI height must tile exactly into {GRID} cells");
        assert_eq!(cell_means(&blank(10), W, &r).len(), GRID * GRID);
    }

    #[test]
    fn response_is_on_minus_off() {
        let r = roi();
        let d = mean_roi(&blank(60), W, &r) - mean_roi(&blank(1), W, &r);
        assert!((d - 59.0).abs() < 1e-9);
    }

    /// The whole point of the falloff test: a curved reflector must score higher
    /// than a flat one. If this ever fails, the print-rejection claim in wiki/05 has
    /// no mechanism behind it.
    #[test]
    fn domed_response_has_higher_falloff_cv_than_flat() {
        let r = roi();
        let dark = blank(0);

        let flat_d: Vec<f64> = cell_means(&flat(60), W, &r)
            .iter()
            .zip(&cell_means(&dark, W, &r))
            .map(|(a, b)| a - b)
            .collect();
        let domed_d: Vec<f64> = cell_means(&domed(60, 10), W, &r)
            .iter()
            .zip(&cell_means(&dark, W, &r))
            .map(|(a, b)| a - b)
            .collect();

        let flat_cv = falloff_cv(&flat_d);
        let domed_cv = falloff_cv(&domed_d);

        assert!(flat_cv < 0.01, "flat surface should be near-uniform, got CV {flat_cv}");
        assert!(domed_cv > flat_cv * 10.0, "domed {domed_cv} vs flat {flat_cv}");
    }

    #[test]
    fn domed_response_is_centre_bright_and_flat_is_not() {
        let r = roi();
        let dark = blank(0);

        let flat_d: Vec<f64> = cell_means(&flat(60), W, &r)
            .iter()
            .zip(&cell_means(&dark, W, &r))
            .map(|(a, b)| a - b)
            .collect();
        let domed_d: Vec<f64> = cell_means(&domed(60, 10), W, &r)
            .iter()
            .zip(&cell_means(&dark, W, &r))
            .map(|(a, b)| a - b)
            .collect();

        assert!((centre_edge_ratio(&flat_d) - 1.0).abs() < 0.01);
        assert!(centre_edge_ratio(&domed_d) > 1.2);
    }

    /// Absolute brightness must not change the falloff verdict -- AGC droop moves it
    /// by 13% within a single capture and 10x between sessions. wiki/10 Probe 3b.
    #[test]
    fn falloff_cv_is_invariant_to_overall_gain() {
        let r = roi();
        let dark = blank(0);
        let cv_at = |peak: u8, floor: u8| {
            let d: Vec<f64> = cell_means(&domed(peak, floor), W, &r)
                .iter()
                .zip(&cell_means(&dark, W, &r))
                .map(|(a, b)| a - b)
                .collect();
            falloff_cv(&d)
        };
        // Same shape, half the gain.
        let bright = cv_at(120, 20);
        let dim = cv_at(60, 10);
        assert!(
            (bright - dim).abs() < 0.02,
            "gain must not move CV: {bright} vs {dim}"
        );
    }

    #[test]
    fn classify_detects_a_strobe() {
        let means: Vec<f64> = (0..30)
            .map(|i| if i % 2 == 0 { 60.0 } else { 0.004 })
            .collect();
        let (phases, tier) = classify(&means);
        assert_eq!(tier, Tier::IrStrobe);
        assert_eq!(phases[0], Phase::On);
        assert_eq!(phases[1], Phase::Off);
    }

    /// The property the whole difference-domain view exists for: adding an ambient
    /// pedestal to *both* phases must not change the measured strobe at all.
    ///
    /// This is the daylight case reduced to arithmetic. The ratio test calls the first
    /// series a strobe and the second constant illumination; the emitter is identical in
    /// both. If this test ever fails, the new statistic has picked up the old one's bug.
    #[test]
    fn alternation_is_invariant_to_an_ambient_pedestal() {
        let night: Vec<f64> = (0..30).map(|i| if i % 2 == 0 { 14.004 } else { 0.004 }).collect();
        let daylight: Vec<f64> = night.iter().map(|m| m + 93.0).collect();

        let n = alternation(&night).expect("enough frames");
        let d = alternation(&daylight).expect("enough frames");

        assert!((n.amplitude - d.amplitude).abs() < 1e-9, "{} vs {}", n.amplitude, d.amplitude);
        assert!((n.sign_regularity - d.sign_regularity).abs() < 1e-9);
        assert_eq!(n.snr, d.snr, "SNR must not depend on the ambient floor");

        // And the ratio test disagrees with itself across exactly this pair, which is the
        // bug being worked around.
        assert_eq!(classify(&night).1, Tier::IrStrobe);
        assert_eq!(classify(&daylight).1, Tier::IrConstant);
    }

    /// Modulation *should* collapse under a pedestal -- it is the fraction of the signal
    /// the emitter accounts for. Reported precisely so the two are not confused: it is the
    /// quantity that legitimately got worse, while amplitude and SNR did not.
    #[test]
    fn modulation_collapses_under_a_pedestal_even_though_snr_does_not() {
        let night: Vec<f64> = (0..30).map(|i| if i % 2 == 0 { 14.0 } else { 0.0 }).collect();
        let daylight: Vec<f64> = night.iter().map(|m| m + 93.0).collect();
        let n = alternation(&night).expect("enough frames");
        let d = alternation(&daylight).expect("enough frames");
        assert!(n.modulation > 10.0 * d.modulation, "{} vs {}", n.modulation, d.modulation);
    }

    /// A sensor that is merely drifting, with no emitter, must not look like a strobe:
    /// its steps do not reverse sign.
    #[test]
    fn a_monotonic_drift_is_not_an_alternation() {
        let drifting: Vec<f64> = (0..30).map(|i| 100.0 - i as f64 * 0.4).collect();
        let a = alternation(&drifting).expect("enough frames");
        assert!(a.sign_regularity < 0.1, "drift read as alternating: {}", a.sign_regularity);
    }

    #[test]
    fn classify_detects_constant_illumination() {
        // AGC wobble on a steadily-lit sensor must not read as a strobe.
        let means: Vec<f64> = (0..30).map(|i| 55.0 + (i % 3) as f64).collect();
        let (_, tier) = classify(&means);
        assert_eq!(tier, Tier::IrConstant);
    }

    #[test]
    fn classify_detects_a_dark_emitter() {
        let means: Vec<f64> = (0..30).map(|i| 0.004 + (i % 3) as f64 * 0.0001).collect();
        let (_, tier) = classify(&means);
        assert_eq!(tier, Tier::EmitterDark);
    }

    #[test]
    fn end_to_end_on_a_synthetic_strobe_capture() {
        // 20 frames alternating a domed "face" with a near-dark reference.
        let mut bytes = Vec::new();
        for i in 0..20 {
            if i % 2 == 0 {
                bytes.extend_from_slice(&domed(60, 10));
            } else {
                bytes.extend_from_slice(&blank(0));
            }
        }
        let cap = Capture::new(bytes, W, H).expect("well-formed capture");
        assert_eq!(cap.tier, Tier::IrStrobe);
        assert!(!cap.pairs.is_empty(), "must find ON/OFF pairs");
        for p in &cap.pairs {
            assert!(p.d > 0.0, "response must be positive against a dark frame");
            assert!(p.dark_ratio < 0.01, "dark frame must be dark relative to ON");
            assert!(p.centre_edge > 1.2, "domed source must read centre-bright");
        }
        // No frame may be used by two pairs.
        assert!(cap.pairs.len() <= 20 / 2);
    }

    #[test]
    fn rejects_a_file_that_is_not_whole_frames() {
        let bytes = vec![0u8; W * H + 7];
        assert!(Capture::new(bytes, W, H).is_err());
    }

    #[test]
    fn rejects_a_file_shorter_than_one_frame() {
        assert!(Capture::new(vec![0u8; 10], W, H).is_err());
    }

    #[test]
    fn percentiles_pick_the_right_ranks() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&xs, 0.0), 1.0);
        assert_eq!(percentile(&xs, 0.5), 3.0);
        assert_eq!(percentile(&xs, 1.0), 5.0);
    }
}
