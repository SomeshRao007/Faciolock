use std::path::Path;
use std::sync::Arc;

use tract_onnx::prelude::*;

use crate::config::DetectConfig;
use crate::frame::{Frame, Rect};

const OUTPUT_NAMES: [&str; 12] = [
    "cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16", "bbox_32",
    "kps_8", "kps_16", "kps_32",
];

const STRIDES: [usize; 3] = [8, 16, 32];

const PAD_VALUE: f32 = 0.0;

pub const LANDMARKS: usize = 5;

#[derive(Debug)]
pub enum DetectError {
    Load(String),
    UnexpectedOutputs(Vec<String>),
    Run(String),
    Geometry { width: usize, height: usize },
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectError::Load(e) => write!(f, "model load failed: {e}"),
            DetectError::UnexpectedOutputs(got) => {
                write!(f, "expected {OUTPUT_NAMES:?}, model has {got:?}")
            }
            DetectError::Run(e) => write!(f, "inference failed: {e}"),
            DetectError::Geometry { width, height } => {
                write!(f, "frame {width}x{height} is not usable")
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Detection {
    pub bounds: Rect,
    pub landmarks: [(f32, f32); LANDMARKS],
    pub score: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Letterbox {
    pub scale: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub side: usize,
}

impl Letterbox {
    pub fn fit(width: usize, height: usize, side: usize) -> Letterbox {
        let scale = (side as f32 / width as f32).min(side as f32 / height as f32);
        let inner_w = (width as f32 * scale).round();
        let inner_h = (height as f32 * scale).round();
        Letterbox {
            scale,
            pad_x: ((side as f32 - inner_w) / 2.0).floor(),
            pad_y: ((side as f32 - inner_h) / 2.0).floor(),
            side,
        }
    }

    pub fn invert_point(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.pad_x) / self.scale, (y - self.pad_y) / self.scale)
    }

    pub fn invert_rect(&self, r: &Rect) -> Rect {
        let (x, y) = self.invert_point(r.x, r.y);
        Rect { x, y, w: r.w / self.scale, h: r.h / self.scale }
    }
}

fn letterbox_into_tensor(frame: &Frame<'_>, lb: &Letterbox) -> Tensor {
    let side = lb.side;
    let mut plane = vec![PAD_VALUE; side * side];

    let inner_w = (frame.width as f32 * lb.scale).round() as usize;
    let inner_h = (frame.height as f32 * lb.scale).round() as usize;

    for oy in 0..inner_h {
        let sy = ((oy as f32 + 0.5) / lb.scale) - 0.5;
        let sy0 = sy.floor();
        let fy = sy - sy0;
        let y0 = (sy0 as isize).clamp(0, frame.height as isize - 1) as usize;
        let y1 = ((sy0 as isize) + 1).clamp(0, frame.height as isize - 1) as usize;

        for ox in 0..inner_w {
            let sx = ((ox as f32 + 0.5) / lb.scale) - 0.5;
            let sx0 = sx.floor();
            let fx = sx - sx0;
            let x0 = (sx0 as isize).clamp(0, frame.width as isize - 1) as usize;
            let x1 = ((sx0 as isize) + 1).clamp(0, frame.width as isize - 1) as usize;

            let p00 = frame.pixels[y0 * frame.width + x0] as f32;
            let p01 = frame.pixels[y0 * frame.width + x1] as f32;
            let p10 = frame.pixels[y1 * frame.width + x0] as f32;
            let p11 = frame.pixels[y1 * frame.width + x1] as f32;

            let top = p00 + (p01 - p00) * fx;
            let bottom = p10 + (p11 - p10) * fx;

            let dy = oy + lb.pad_y as usize;
            let dx = ox + lb.pad_x as usize;
            plane[dy * side + dx] = top + (bottom - top) * fy;
        }
    }

    let mut data = Vec::with_capacity(3 * side * side);
    for _ in 0..3 {
        data.extend_from_slice(&plane);
    }
    Tensor::from_shape(&[1, 3, side, side], &data).expect("shape matches the buffer length")
}

pub struct Detector {
    model: Arc<TypedSimplePlan>,
    input_size: usize,
    score_min: f32,
    nms_iou_max: f32,
}

impl Detector {
    pub fn load(path: &Path, cfg: &DetectConfig) -> Result<Detector, DetectError> {
        let side = cfg.input_size;
        let parsed = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| DetectError::Load(format!("{e:?}")))?;

        let names = output_labels(&parsed);
        if names != OUTPUT_NAMES {
            return Err(DetectError::UnexpectedOutputs(names));
        }

        let model = parsed
            .with_input_fact(0, f32::fact([1, 3, side, side]).into())
            .map_err(|e| DetectError::Load(format!("{e:?}")))?
            .into_optimized()
            .map_err(|e| DetectError::Load(format!("{e:?}")))?
            .into_runnable()
            .map_err(|e| DetectError::Load(format!("{e:?}")))?;

        Ok(Detector {
            model,
            input_size: side,
            score_min: cfg.score_min,
            nms_iou_max: cfg.nms_iou_max,
        })
    }

    pub fn input_size(&self) -> usize {
        self.input_size
    }

    pub fn detect(&self, frame: &Frame<'_>) -> Result<Vec<Detection>, DetectError> {
        if frame.width == 0 || frame.height == 0 {
            return Err(DetectError::Geometry { width: frame.width, height: frame.height });
        }
        let lb = Letterbox::fit(frame.width, frame.height, self.input_size);
        let input = letterbox_into_tensor(frame, &lb);

        let outputs = self
            .model
            .run(tvec!(input.into()))
            .map_err(|e| DetectError::Run(format!("{e:?}")))?;

        let mut views: Vec<&[f32]> = Vec::with_capacity(OUTPUT_NAMES.len());
        for o in outputs.iter() {
            views.push(
                o.view().as_slice::<f32>().map_err(|e| DetectError::Run(format!("{e:?}")))?,
            );
        }

        let mut candidates = Vec::new();
        for (s, &stride) in STRIDES.iter().enumerate() {
            let cols = self.input_size / stride;
            let rows = self.input_size / stride;
            let cls = views[s];
            let obj = views[3 + s];
            let bbox = views[6 + s];
            let kps = views[9 + s];

            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    if i >= cls.len() || i >= obj.len() {
                        continue;
                    }
                    let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                    if score < self.score_min {
                        continue;
                    }
                    if (i + 1) * 4 > bbox.len() || (i + 1) * 10 > kps.len() {
                        continue;
                    }

                    let b = &bbox[i * 4..i * 4 + 4];
                    let cx = (c as f32 + b[0]) * stride as f32;
                    let cy = (r as f32 + b[1]) * stride as f32;
                    let w = b[2].exp() * stride as f32;
                    let h = b[3].exp() * stride as f32;
                    let model_rect = Rect { x: cx - w / 2.0, y: cy - h / 2.0, w, h };

                    let k = &kps[i * 10..i * 10 + 10];
                    let mut landmarks = [(0.0f32, 0.0f32); LANDMARKS];
                    for (n, lm) in landmarks.iter_mut().enumerate() {
                        let mx = (c as f32 + k[2 * n]) * stride as f32;
                        let my = (r as f32 + k[2 * n + 1]) * stride as f32;
                        *lm = lb.invert_point(mx, my);
                    }

                    candidates.push(Detection {
                        bounds: lb.invert_rect(&model_rect),
                        landmarks,
                        score,
                    });
                }
            }
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(nms(candidates, self.nms_iou_max))
    }

    pub fn detect_best(&self, frame: &Frame<'_>) -> Result<Option<Detection>, DetectError> {
        Ok(self.detect(frame)?.into_iter().next())
    }
}

fn nms(sorted: Vec<Detection>, iou_max: f32) -> Vec<Detection> {
    let mut kept: Vec<Detection> = Vec::new();
    for cand in sorted {
        if kept.iter().any(|k| k.bounds.iou(&cand.bounds) > iou_max) {
            continue;
        }
        kept.push(cand);
    }
    kept
}

fn output_labels(model: &InferenceModel) -> Vec<String> {
    model
        .output_outlets()
        .map(|outlets| {
            outlets
                .iter()
                .map(|o| {
                    model
                        .outlet_label(*o)
                        .map(str::to_string)
                        .unwrap_or_else(|| model.node(o.node).name.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{domed, H, W};

    #[test]
    fn letterbox_maps_a_point_and_maps_it_back() {
        let lb = Letterbox::fit(640, 360, 192);
        assert!((lb.scale - 0.3).abs() < 1e-6);
        assert_eq!(lb.pad_x, 0.0);
        assert!(lb.pad_y > 0.0);

        let (fx, fy) = lb.invert_point(96.0, 96.0);
        assert!((fx - 320.0).abs() < 0.5, "centre of the square is the centre of the frame");
        assert!((fy - 180.0).abs() < 1.5);
    }

    #[test]
    fn letterbox_of_a_square_frame_has_no_padding() {
        let lb = Letterbox::fit(360, 360, 192);
        assert_eq!(lb.pad_x, 0.0);
        assert_eq!(lb.pad_y, 0.0);
    }

    #[test]
    fn letterboxed_tensor_has_the_declared_shape_and_padding() {
        let px = domed(W, H, 200, 20);
        let frame = Frame::new(&px, W, H).unwrap();
        let lb = Letterbox::fit(W, H, 96);
        let t = letterbox_into_tensor(&frame, &lb);
        assert_eq!(t.shape(), &[1, 3, 96, 96]);

        let v = t.view().as_slice::<f32>().unwrap();
        let plane = 96 * 96;
        assert_eq!(&v[0..plane], &v[plane..2 * plane]);
        assert_eq!(&v[0..plane], &v[2 * plane..3 * plane]);
    }

    #[test]
    fn letterbox_margin_holds_the_pad_value() {
        let px = crate::testing::blank(64, 32, 200);
        let frame = Frame::new(&px, 64, 32).unwrap();
        let lb = Letterbox::fit(64, 32, 64);
        let t = letterbox_into_tensor(&frame, &lb);
        let v = t.view().as_slice::<f32>().unwrap();
        assert_eq!(v[0], PAD_VALUE, "top-left corner is margin");
        assert_eq!(v[63], PAD_VALUE);
    }

    #[test]
    fn nms_drops_an_overlapping_lower_scoring_box() {
        let mk = |x: f32, score: f32| Detection {
            bounds: Rect { x, y: 0.0, w: 10.0, h: 10.0 },
            landmarks: [(0.0, 0.0); LANDMARKS],
            score,
        };
        let kept = nms(vec![mk(0.0, 0.9), mk(1.0, 0.8), mk(50.0, 0.7)], 0.3);
        assert_eq!(kept.len(), 2, "the near-duplicate is suppressed, the distant box is not");
        assert!((kept[0].score - 0.9).abs() < 1e-6);
    }
}
