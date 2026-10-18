#!/usr/bin/env python3
"""Generate golden references for `faciolock-vision` from OpenCV.

A golden generated from our own implementation only catches *drift*. It cannot catch an
implementation that was wrong from the first commit, and a subtly wrong face pipeline is
the worst kind: it produces plausible boxes with skewed landmarks, every stage downstream
still runs, and the only symptom is a quietly worse false-reject rate. So the goldens come
from an independent implementation -- OpenCV's own `FaceDetectorYN`, reading the same ONNX
file, and OpenCV's own similarity-transform estimator.

Two artifacts, deliberately separated by whether they contain biometrics:

  testdata/align_golden.txt      SYNTHETIC. Committed, runs in CI, no face anywhere.
  testdata/align_golden_chip.bin A deterministic integer pattern warped through known
                                 landmarks. Validates the similarity transform and the warp
                                 -- i.e. that our reference points, landmark order and
                                 transform direction match ArcFace's conventions.

  testdata/detect_golden.txt     DERIVED FROM A REAL FACE. Gitignored, local only.
                                 Box and landmarks for each corpus capture, so our
                                 12-tensor YuNet decode can be checked against OpenCV's.

Line-oriented rather than JSON so the Rust side parses them with `split_whitespace` and
the crate stays on exactly one dependency.

Usage (needs a venv with opencv-python and numpy, not the system python):
    <venv>/bin/python tools/gen-goldens.py
"""

import json
import pathlib
import sys

import cv2
import numpy as np

ROOT = pathlib.Path(__file__).resolve().parent.parent
MODELS = ROOT / "models"
CORPUS = ROOT / "corpus"
TESTDATA = ROOT / "crates" / "faciolock-vision" / "testdata"

YUNET = MODELS / "face_detection_yunet_2026may.onnx"

# Must match crates/faciolock-vision/src/config.rs and detect.rs.
INPUT_SIZE = 320
SCORE_MIN = 0.3
NMS_IOU = 0.3
PAD_VALUE = 0

# Must match crates/faciolock-vision/src/align.rs REFERENCE_POINTS.
REFERENCE_POINTS = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float64,
)
CHIP_SIZE = 112

# Synthetic source for the alignment golden. Integer arithmetic so Python and Rust
# generate byte-identical input without shipping an image.
#
# A checkerboard, and the block sizes differ in x and y on purpose:
#
#   * mostly flat, so a sub-pixel difference between OpenCV's fixed-point bilinear and
#     our f32 one changes nothing across the interior of a block -- the comparison is
#     about the transform, not about interpolation rounding;
#   * high contrast at block edges, so a wrong transform moves those edges wholesale and
#     the difference is enormous rather than subtle;
#   * 16 wide by 24 tall breaks the 90-degree rotational symmetry a square checkerboard
#     would have, which is what makes a transposed or rotated chip detectable.
#
# The first version of this used (x*7 + y*13) % 256. It wraps, so a half-pixel sampling
# difference next to a wrap boundary swings ~250 levels and the comparison measured the
# discontinuity rather than the warp.
PATTERN_W = 160
PATTERN_H = 160


def pattern():
    x = np.arange(PATTERN_W, dtype=np.int64)[None, :]
    y = np.arange(PATTERN_H, dtype=np.int64)[:, None]
    return np.where((x // 16 + y // 24) % 2 == 0, 40, 215).astype(np.uint8)


def letterbox(gray, side):
    """Mirror of detect.rs Letterbox::fit -- aspect-preserving, centred, pad 0."""
    h, w = gray.shape
    scale = min(side / w, side / h)
    inner_w = int(round(w * scale))
    inner_h = int(round(h * scale))
    pad_x = int((side - inner_w) // 2)
    pad_y = int((side - inner_h) // 2)
    out = np.full((side, side), PAD_VALUE, dtype=np.uint8)
    resized = cv2.resize(gray, (inner_w, inner_h), interpolation=cv2.INTER_LINEAR)
    out[pad_y : pad_y + inner_h, pad_x : pad_x + inner_w] = resized
    return out, scale, float(pad_x), float(pad_y)


def read_capture(gray_path, sidecar_path):
    meta = json.loads(sidecar_path.read_text())
    w, h = meta["width"], meta["height"]
    buf = np.fromfile(gray_path, dtype=np.uint8)
    frames = buf.reshape(-1, h, w)
    return meta, frames


def brightest_index(frames, lo=5, hi=20):
    hi = min(hi, len(frames))
    lo = min(lo, hi - 1)
    means = [frames[i].mean() for i in range(lo, hi)]
    return lo + int(np.argmax(means))


def align_golden():
    """Warp a synthetic pattern through known landmarks, using OpenCV's estimator."""
    src_img = pattern()

    # Landmarks in a plausible face arrangement, chosen to exercise rotation and scale
    # rather than a degenerate axis-aligned case.
    landmarks = np.array(
        [
            [52.0, 61.0],
            [104.0, 67.0],
            [76.0, 89.0],
            [56.0, 116.0],
            [101.0, 121.0],
        ],
        dtype=np.float64,
    )

    m, inliers = cv2.estimateAffinePartial2D(
        landmarks.reshape(-1, 1, 2).astype(np.float32),
        REFERENCE_POINTS.reshape(-1, 1, 2).astype(np.float32),
        method=cv2.LMEDS,
    )
    if m is None:
        raise SystemExit("OpenCV could not estimate the similarity transform")

    chip = cv2.warpAffine(
        src_img, m, (CHIP_SIZE, CHIP_SIZE), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE
    )

    lines = [
        "# SYNTHETIC -- no biometric data. Committed and run in CI.",
        "# Source is a 16x24 checkerboard, generated identically in Rust,",
        f"# so no image needs shipping. Transform and chip from OpenCV {cv2.__version__}.",
        f"opencv {cv2.__version__}",
        f"pattern {PATTERN_W} {PATTERN_H}",
    ]
    for lx, ly in landmarks:
        lines.append(f"landmark {float(lx)!r} {float(ly)!r}")
    for rx, ry in REFERENCE_POINTS:
        lines.append(f"reference {float(rx)!r} {float(ry)!r}")
    # a, -b, tx, b, a, ty  -- row-major 2x3, exactly as OpenCV returns it.
    flat = [float(v) for row in m for v in row]
    lines.append("transform " + " ".join(repr(v) for v in flat))
    lines.append(f"chip_size {CHIP_SIZE}")

    (TESTDATA / "align_golden_chip.bin").write_bytes(chip.tobytes())
    return "\n".join(lines) + "\n"


def detect_goldens():
    """Box and landmarks per corpus capture, from OpenCV's own YuNet decode."""
    detector = cv2.FaceDetectorYN.create(
        str(YUNET), "", (INPUT_SIZE, INPUT_SIZE), SCORE_MIN, NMS_IOU, 5000
    )

    out = {}
    for sidecar in sorted(CORPUS.glob("*.json")):
        gray_path = sidecar.with_suffix(".gray")
        if not gray_path.exists():
            print(f"  skip {sidecar.name}: no pixels beside it")
            continue
        meta, frames = read_capture(gray_path, sidecar)
        idx = brightest_index(frames)
        frame = frames[idx]

        square, scale, pad_x, pad_y = letterbox(frame, INPUT_SIZE)
        bgr = cv2.cvtColor(square, cv2.COLOR_GRAY2BGR)

        detector.setInputSize((INPUT_SIZE, INPUT_SIZE))
        _, faces = detector.detect(bgr)

        record = {"frame_index": idx, "faces": []}
        if faces is not None:
            for f in faces:
                # OpenCV row: x, y, w, h, then 5 landmark pairs, then score.
                x, y, w, h = (float(v) for v in f[0:4])
                lms = [
                    [(float(f[4 + 2 * i]) - pad_x) / scale, (float(f[5 + 2 * i]) - pad_y) / scale]
                    for i in range(5)
                ]
                record["faces"].append(
                    {
                        "bounds": [
                            (x - pad_x) / scale,
                            (y - pad_y) / scale,
                            w / scale,
                            h / scale,
                        ],
                        "landmarks": lms,
                        "score": float(f[14]),
                    }
                )
        out[gray_path.stem] = record
        n = len(record["faces"])
        print(f"  {gray_path.stem:<44} frame {idx:>3}  {n} face(s)")

    lines = [
        "# DERIVED FROM REAL FACES -- gitignored, local only.",
        "# Regenerate with tools/gen-goldens.py. Coordinates are full-frame.",
        "# Keyed by filename stem, not label: two takes of the same scene share a label.",
        f"opencv {cv2.__version__}",
        f"input_size {INPUT_SIZE}",
        f"score_min {SCORE_MIN!r}",
    ]
    for label, rec in out.items():
        lines.append(f"capture {label} {rec['frame_index']} {len(rec['faces'])}")
        for f in rec["faces"]:
            vals = list(f["bounds"]) + [f["score"]] + [c for p in f["landmarks"] for c in p]
            lines.append("face " + " ".join(repr(float(v)) for v in vals))
    return "\n".join(lines) + "\n"


def main():
    if not YUNET.exists():
        raise SystemExit(f"missing model: {YUNET}")
    TESTDATA.mkdir(parents=True, exist_ok=True)

    print("alignment golden (synthetic, committed)")
    (TESTDATA / "align_golden.txt").write_text(align_golden())
    print(f"  wrote {TESTDATA / 'align_golden.txt'} + align_golden_chip.bin")

    print("detection goldens (real faces, gitignored)")
    if not CORPUS.exists():
        print("  no corpus directory; skipping")
        return
    (TESTDATA / "detect_golden.txt").write_text(detect_goldens())
    print(f"  wrote {TESTDATA / 'detect_golden.txt'}")


if __name__ == "__main__":
    sys.exit(main())
