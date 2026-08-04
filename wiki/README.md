# Faciolock — Knowledge Wiki

IR face authentication for Linux via PAM. Rust. Built to look and behave like a
platform component, not an add-on.

**Status: Phase 1 built.** `crates/faciolock-vision` takes frames in and produces a
verdict out — liveness, YuNet detection, alignment, SFace embedding, k-of-n matching — with
no camera, no sockets and no PAM. 78 tests, of which the ones that need neither the
recordings nor the models always run. Detection and alignment are pinned against
independent OpenCV references, which is what caught [B16](11-bug-log.md#b16).

## Index

| Page | What's in it |
|---|---|
| [01 — Goals & Scope](01-goals-and-scope.md) | What we're building, what we're explicitly not |
| [02 — Howdy Teardown](02-howdy-teardown.md) | How Howdy actually works, measured weaknesses |
| [03 — PAM & Security Model](03-pam-and-security.md) | PAM contract, threat model, attack surface |
| [04 — Architecture](04-architecture.md) | Daemon + thin PAM module + CLI, privilege split |
| [05 — Vision Pipeline](05-vision-pipeline.md) | Camera, IR, models, licenses, liveness, matching |
| [06 — Performance Budget](06-performance-budget.md) | Latency/memory targets and where they go |
| [07 — Desktop & Packaging](07-desktop-and-packaging.md) | KDE/SDDM/sudo, Debian/Ubuntu upstreaming |
| [08 — Open Questions](08-open-questions.md) | Decisions needed before any code is written |
| [09 — Roadmap](09-roadmap.md) | 8 phases, exit criteria, risk register |
| [10 — Hardware Probe Log](10-hardware-probe.md) | Exact commands + raw output from the reference machine |
| [11 — Bug Log](11-bug-log.md) | Bugs hit and fixed, with the design changes they forced |

## One-paragraph summary

A privilege-separated design copied from `fprintd`: a small, unprivileged,
socket-activated daemon owns the camera and the ML; a ~400-line PAM module owns
nothing but a Unix socket. Detection and embedding run on permissively-licensed
ONNX models (YuNet MIT / SFace Apache-2.0) so the whole thing is redistributable by
Debian — EdgeFace was ruled out when its *weights* turned out to be non-commercial
([B10](11-bug-log.md#b10)). Liveness is arithmetic, not a neural net: the IR
camera already alternates emitter-lit and dark frames, so differencing them rejects
screens, prints, and IR-floodlight attacks for the cost of a few `f32` reductions and
no writes to the device. Enrolled templates are sealed to the TPM. Target: p95 unlock
under 400 ms wall-clock, zero RSS when idle.

## Decisions so far

| # | Decision | Date |
|---|---|---|
| Q6 | Debian/Ubuntu archive is the goal; design constrained accordingly | 2026-08-03 |
| ~~Q1~~ | ~~EdgeFace-XS for embedding~~ — **reversed, weights are non-commercial** | ~~2026-08-03~~ |
| Q1 | **SFace** for embedding (Apache 2.0). EdgeFace ruled out — [B10](11-bug-log.md#b10) | 2026-08-04 |
| Q2 | **`tract`** for inference — zero native deps beat `ort`'s 118 ms | 2026-08-04 |
| Q3 | No emitter control code — hardware self-strobes, verified by measurement | 2026-08-03 |
| Q7 | v1 scope is the **full vertical** — see [09](09-roadmap.md) | 2026-08-03 |
| P1-a | Matching is **2-of-3**; [06](06-performance-budget.md)'s budget restated to match | 2026-08-04 |
| P1-b | Uncalibrated liveness checks enforce a **degenerate floor only**, and report `enforced` vs `advisory` | 2026-08-04 |
| P1-c | CI corpus is **synthetic and committed**; the real recordings stay local | 2026-08-04 |
| P1-e | Capability tier is **enrolment-time config, never inferred per burst** | 2026-08-04 |
| P1-f | Detector runs at **320×320** — measured on embedding accuracy, not latency ([B13](11-bug-log.md#b13)) | 2026-08-04 |

Still open: Q4 (IPC), Q5 (TPM timing), Q9 (name), and whether the real recordings are ever
published (deferred to ITP — publishing is irreversible, so it is not decided early).

## Reference hardware

ASUS Vivobook. IR camera at `/dev/video2`: `Y800` greyscale, 640×360 @ 30 fps,
`rawvideo` (no MJPEG decode needed), hardware-strobed emitter at 15 ON/OFF pairs per
second. Commands and raw output: [10](10-hardware-probe.md). Interpretation:
[05](05-vision-pipeline.md#measured-the-hardware-strobes-for-us).

Addressed by **`by-path`** (`…-usb-0:7:1.2-video-index0`). `by-id` collides on this
hardware and resolves to the RGB camera —
[10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation).

## Non-negotiables

1. No ML, no image decoding, no camera I/O inside the PAM module's address space.
2. Fail closed. Any error path returns a PAM failure code, never `PAM_SUCCESS`.
3. Every shipped model file must be redistributable under a DFSG-compatible license.
4. Never the sole authentication factor; password fallback always reachable.
5. **Liveness aggregates conjunctively — every pair in the burst must pass.** Matching
   aggregates k-of-n. Unifying the two reintroduces a measured 1-in-36 bypass
   ([B9](11-bug-log.md#b9)).

## Verified so far

| Claim | Status |
|---|---|
| Hardware self-strobes, 15 ON/OFF pairs/sec | **measured**, 3 independent runs |
| Dark-frame integrity rejects an IR floodlight | **verified** — 4 orders of magnitude margin, 0 false rejects in 210 live pairs |
| One contaminated pair sinks a burst (conjunctive aggregation) | **verified** — replayed in CI on synthetic and recorded captures |
| YuNet detects a face in **near-infrared** | **verified** — 4/4 live captures, 0 on an empty room, 0 on the floodlight attack |
| SFace embeddings discriminate on **near-infrared** | **verified** — genuine cross-capture cosine 0.60–0.90 against a 0.363 threshold |
| Falloff is measurable over a real face box | **verified** — 0.266–0.313, [B7 closed](11-bug-log.md#b7) |
| ~~Response test rejects screens~~ | **refuted** — a phone reads D=36 against a live minimum of 48.6, not ~0. The statistic is whole-frame, so it measures the room ([B17](11-bug-log.md#b17)) |
| A phone screen showing a photo is rejected | **verified** — but by *detection* (`NoFace`), not by any liveness check |
| **Falloff test rejects printed photos** | **untested, and deliberately deferred (2026-08-04).** [B7](11-bug-log.md#b7) is closed; no print has been recorded. The check therefore enforces only a degenerate-flatness floor and claims nothing about prints |
| False accept rate | **unmeasurable** — the corpus has one subject. The 0.363 cosine threshold is paper-derived |
