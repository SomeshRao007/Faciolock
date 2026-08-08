# Faciolock — Knowledge Wiki

IR face authentication for Linux via PAM. Rust. Built to look and behave like a
platform component, not an add-on.

**Status: Phase 1 built.** `crates/faciolock-vision` takes frames in and produces a
verdict out — liveness, YuNet detection, alignment, SFace embedding, k-of-n matching — with
no camera, no sockets and no PAM. 78 tests, of which the ones that need neither the
recordings nor the models always run. Detection and alignment are pinned against
independent OpenCV references, which is what caught [B16](11-bug-log.md#b16).

**Phase 2 in progress.** `crates/faciolock-camera` (V4L2 capture, IR discovery) and
`crates/faciolock-ipc` (the wire types) are built. First contact with real hardware found
[B18](11-bug-log.md#b18) — every strobe statistic was a ratio, and ambient daylight
saturates a ratio — which is now fixed by moving to the difference domain and by an
**ambient gate**: above a measured floor this hardware reports "cannot verify" and PAM
falls through to the password, rather than guessing. That is the honest ceiling, and it
is where the class sits: Windows Hello fails the same way with purpose-built optics, and
Howdy passes in daylight by checking nothing.

The frame burst is now **measured at 404 ms** against a 130 ms estimate
([B19](11-bug-log.md#b19)), so the warm path is ~673 ms against a 400 ms target and
[06](06-performance-budget.md) is restated.

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
| P2-a | **`v4l` crate** for capture, not hand-rolled ioctls — correct struct layouts on architectures that cannot be tested here | 2026-08-06 |
| P2-b | **`toml` + `serde`** for daemon config; both already in the archive, so no packaging cost | 2026-08-06 |
| P2-c | **Fresh frames only** — camera stays warm and streaming, queue flushed at request time | 2026-08-06 |
| P2-d | **Copy-out, not zero-copy** — 1.4 MB and ~150 µs against a 400 ms budget; the memory-budget argument for zero-copy was wrong by three orders of magnitude | 2026-08-06 |
| P2-e | **Strobe statistics move to the difference domain**; a ratio saturates against ambient IR ([B18](11-bug-log.md#b18)) | 2026-08-06 |
| P2-f | **Ambient gate at floor 0.04**, before any biometric work. Above it the answer is "cannot verify", not a guess | 2026-08-06 |
| P2-g | **`Environment` is exempt from the failure budget** — weather must not disable face unlock | 2026-08-06 |
| P2-h | **No RGB camera in v1.** Measured: it cannot see the emitter (SNR 0.85 vs the IR camera's 1,500–15,000), so it cannot do strobe liveness. AND-ing it in would rely on a branch that reads *true* for attackers in daylight; OR-ing it in lets a photo unlock. Deferred to v2 as an explicit opt-in tier | 2026-08-06 |

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

> **Every capture below except the daylight ones was recorded after dark, and until
> 2026-08-06 nothing recorded that fact.** Ambient near-infrared turned out to be the
> variable the whole corpus was uniform in, and it breaks every ratio-domain statistic
> ([B18](11-bug-log.md#b18)). Rows are qualified accordingly. A claim measured in one
> lighting condition is a claim about that condition.

| Claim | Status |
|---|---|
| Hardware self-strobes, every frame | **measured**, 4 independent runs, and now in daylight too — the emitter never stops |
| ~~15 ON/OFF pairs/sec~~ | **withdrawn.** 30 fps was the *advertised* rate; delivered is 12–15 fps, so ~6–7 pairs/sec ([B19](11-bug-log.md#b19)) |
| Dark-frame integrity rejects an IR floodlight | **verified at negligible ambient IR only** — 4 orders of margin and 0 false rejects across 210 live pairs, all nocturnal. In daylight the same check rejects **100% of honest captures** (0.664 against a 0.001 threshold). The check is a ratio and ambient light is an offset ([B18](11-bug-log.md#b18)) |
| The strobe is detectable in daylight | **verified** — as a *difference*: SNR 549 whole-frame, 882 over the face box, sign regularity 1.000. `classify` was rewritten to measure it ([B18](11-bug-log.md#b18)) |
| Liveness can be **distinguished from a floodlight attack** above an ambient floor of ~1 | **refuted, by a matched pair** — same room, same position, 90 s apart, ambient within 1% (13.91 vs 14.05). Every statistic ranked the attack as *more live-looking than the operator's real face*. A hardware ceiling, not a tuning problem ([B18](11-bug-log.md#b18)) |
| Faciolock works in a normally lit daytime room | **no, and this is now a stated non-goal** ([01](01-goals-and-scope.md)). A large room with indirect sun ~10 m away reads 13.91 — 348× over the gate. It authenticates in dim and artificially-lit rooms and refuses otherwise |
| The RGB camera could supply liveness where IR cannot | **refuted by measurement** — it has an IR-cut filter and cannot see the emitter at all: strobe amplitude 0.049 and SNR 0.85 against the IR camera's 48–81 and 1,500–15,000 |
| YuNet detects a face in **daylight** IR | **verified** — and the face-box emitter response is 2.2× the whole-frame one, as inverse-square predicts |
| One contaminated pair sinks a burst (conjunctive aggregation) | **verified** — replayed in CI on synthetic and recorded captures |
| YuNet detects a face in **near-infrared** | **verified** — 4/4 live captures, 0 on an empty room, 0 on the floodlight attack |
| SFace embeddings discriminate on **near-infrared** | **verified** — genuine cross-capture cosine 0.60–0.90 against a 0.363 threshold |
| Falloff is measurable over a real face box | **verified** — 0.266–0.313, [B7 closed](11-bug-log.md#b7) |
| ~~Response test rejects screens~~ | **refuted** — a phone reads D=36 against a live minimum of 48.6, not ~0. The statistic is whole-frame, so it measures the room ([B17](11-bug-log.md#b17)) |
| A phone screen showing a photo is rejected | **verified** — but by *detection* (`NoFace`), not by any liveness check |
| **Falloff test rejects printed photos** | **untested, and deliberately deferred (2026-08-04).** [B7](11-bug-log.md#b7) is closed; no print has been recorded. The check therefore enforces only a degenerate-flatness floor and claims nothing about prints |
| False accept rate | **unmeasurable** — the corpus has one subject. The 0.363 cosine threshold is paper-derived |
