# Models

ONNX models Faciolock runs. **Not in version control** — they are large binaries fetched
from upstream. They ship as the separate `faciolock-models` package with vendored licences
and a SHA256 manifest (`MANIFEST.sha256`) the daemon verifies before loading any model.

## Licence status

Every model here must be redistributable by Debian: 100% DFSG-compatible licensing.
Two candidates have already been ruled out for failing this, so the bar is:

> A permissive LICENSE in a source repo says nothing about the weights. Check the
> licence **where the weights are distributed**, check the **training data's** terms,
> and check the redistributor had the right to grant it.

| File | Licence | Source | Status |
|---|---|---|---|
| `face_detection_yunet_2023mar.onnx` | MIT | OpenCV Zoo | clean |
| `face_detection_yunet_2026may.onnx` | MIT | OpenCV Zoo | clean — dynamic input shape |
| `face_detection_yunet_2023mar_int8.onnx` | MIT | OpenCV Zoo | clean, but **`tract` cannot load it** (`QLinearAdd`) |
| `face_recognition_sface_2021dec.onnx` | Apache 2.0 | OpenCV Zoo | **conditionally clean — see below** |

### SFace residual risk

OpenCV Zoo ships a per-model `LICENSE` (Apache 2.0) beside the ONNX, which is an
explicit grant on the artifact itself rather than an inference from a repo root. That
covers point 1 of the check above.

Not yet cleared: the upstream [zhongyy/SFace](https://github.com/zhongyy/SFace) repo
declares **no licence at all**, and neither the zoo model card nor the paper states the
training dataset in a way that settles point 2. Face-recognition models are commonly
trained on CASIA-WebFace or MS-Celeb-1M, both research-only.

**This must be resolved before an ITP.** If it does not clear, the fallback is dlib's
`dlib_face_recognition_resnet_model_v1` (**CC0 / public domain**) — lower accuracy
(99.38% LFW) and older, but the only candidate whose licensing is beyond argument. It is
also what Howdy uses, and why Howdy has no licence problem.

### Ruled out

| Model | Why |
|---|---|
| EdgeFace (all variants) | Code BSD-3, **weights CC BY-NC-SA 4.0**. Trained on WebFace260M (research-only). |
| InsightFace `buffalo_l` / ArcFace R100 | Code MIT, **weights non-commercial research only** |

## Fetching

LFS-backed in the zoo, so the plain `raw.githubusercontent.com` URL returns a 131-byte
pointer file rather than a model. Use `media.githubusercontent.com`:

```bash
curl -sL -o models/face_detection_yunet_2026may.onnx \
  https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2026may.onnx
```

A 131-byte "model" is the symptom of getting this wrong.

## Measured under `tract`

Intel Core Ultra 5 225H, single-threaded.

| Model | Input | p50 |
|---|---|---|
| YuNet `2026may` | 160×160 | 7.7 ms |
| YuNet `2026may` | 192×192 | 11.2 ms |
| YuNet `2026may` | 320×320 | 33.9 ms |
| YuNet `2023mar` | 640×640 | 145.0 ms |
| SFace | 112×112 | 85.3 ms |

**YuNet `2026may` requires square inputs.** 640×360 and 160×120 fail under *both*
`tract` and `ort` — it is a limitation of the model's anchor/stride arithmetic, not of
either runtime. The 640×360 frame is letterboxed into a square before detection.
