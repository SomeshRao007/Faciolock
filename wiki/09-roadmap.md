# 09 — Roadmap

**Scope: full vertical.** Everything in [01](01-goals-and-scope.md) gets built.
Estimate ~8 weeks of steady part-time work to a shippable PPA release; the archive
tail ([07](07-desktop-and-packaging.md)) runs well beyond that.

**Ordering principle:** unproven claims first, known engineering last. Two things in
this project could still be wrong — that `tract` can run our models, and that the
falloff test separates a live face from a flat print. Both are answerable in week 1
for near-zero cost, so they go in week 1. Everything after that is work we already
know how to do.

**Status:** the first is **settled** — `tract` runs both models, Q2 decided. The second
is **not**, and is blocked twice over: no print sample, and the falloff test cannot be
measured until detection provides a real face box ([B7](11-bug-log.md#b7)). That moves
YuNet ahead of the liveness thresholds, which is a reordering of Phase 1.

Each phase has an **exit criterion**. A phase isn't done because the code is written;
it's done when the criterion is demonstrated.

---

## Phase 0 — Spikes & corpus (week 1)

Throwaway code. Nothing here survives into the final tree except the corpus.

- **S1 — Runtime spike.** ~~Load YuNet + SFace under `tract` and `ort`, measure.~~
  **Done. [Q2 resolved on `tract`](08-open-questions.md#q2--inference-runtime--decided-tract)**
  — both load, `ort` is 3–20× faster, both meet the budget, decided on zero native
  dependencies and attack surface.
- **S2 — Model licence.** ~~Confirm EdgeFace's BSD-3 covers the trained weights.~~
  **Done, and it does not** — the weights are CC BY-NC-SA 4.0, DFSG-incompatible, so
  Q1 reversed to SFace ([B10](11-bug-log.md#b10)). Remaining: clear SFace's own
  training-data provenance before an ITP.
- **S3 — Capture spike.** Rust + `v4l` on `/dev/video2`, Y800 640×360. Pair ON/OFF
  frames by mean luma. Confirm the alternation from the shell measurement holds from
  our own code.
- **S4 — Corpus recording.** Capture raw ON/OFF bursts of: live face (several angles,
  glasses on/off, lit room, dark room), phone screen, laptop screen, matte print,
  glossy print. **One afternoon of hardware exposure, reused forever.**
- **S5 — Threshold spike.** Compute `D`, ON/OFF ratio, and spatial falloff over the
  corpus. Plot the separation. **The one genuinely unproven claim in the project.**

**Exit:** ~~a number for `tract` vs `ort`~~ **done**; a plot showing live-vs-print
separation with a defensible threshold, or evidence that falloff alone is insufficient
and passive texture checks are also needed — **not done, and not currently reachable.**

---

## Phase 1 — `faciolock-vision` (week 2)

The real vision crate. No camera, no sockets, no PAM — frames in, verdict out.

- Frame pair type, ON/OFF classification
- Liveness: dark-frame integrity, response, falloff, motion consistency
- YuNet detection + 5-landmark extraction
- Similarity-transform alignment to 112×112 — **golden-output regression test**, this
  is the classic silent-drift bug ([05](05-vision-pipeline.md#alignment))
- SFace embedding, cosine matching, best-of-N templates
- Capability tiers: `ir-strobe` / `ir-constant` / `rgb`

**Exit:** `cargo test` replays the whole Phase 0 corpus and asserts correct verdicts.
CI runs it with no camera attached.

---

## Phase 2 — Camera & daemon skeleton (week 3)

- `v4l` capture, MMAP zero-copy streaming, device discovery by **`by-path`** symlink
  (not `by-id` — it collides, see [10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation))
- IR device heuristics: filter to `V4L2_CAP_VIDEO_CAPTURE`, then GREY format → name →
  resolution → chroma variance
- Read-only capability probe (30 frames, test for alternation) — **never writes**
- `faciolockd` skeleton, config loading, model SHA256 verification at startup
- Graceful handling of camera-busy (video call in progress) — reply fast, don't fight

**Exit — restated 2026-08-06, because the original is unmeetable and saying so now is
cheaper than missing it later.** The delivered frame rate is 12–15 fps, so a 6-frame burst
alone spends ~340–410 ms of a 400 ms budget ([B19](11-bug-log.md#b19)), and the warm path
has read ~399 ms against that target since Phase 1 — the "82 ms of headroom" this section
used to quote predates 2-of-3 matching and the 320×320 detector.

1. Frame-burst cost measured in **both ambient regimes**, from kernel timestamps, p50/p95/p99.
2. Correct verdict on real hardware in both regimes — which is blocked on
   [B18](11-bug-log.md#b18), since honest daylight captures currently cannot authenticate.
3. [06](06-performance-budget.md) restated from the measured delivery rate.
4. `faciolockd` answers a hand-rolled socket request with a real verdict from the real camera.

If the burst cost is confirmed at ~340 ms, the latency conversation moves to **overlapping
capture with compute** — SFace's 230 ms can hide inside the capture window — which is a
pipeline change and needs its own decision entry rather than a quiet re-open of Q2.

---

## Phase 3 — PAM module & IPC (week 4)

The security-critical week.

- `faciolock-ipc`: fixed-layout binary protocol, hard size caps
- `faciolock-pam` cdylib: `pam_sm_authenticate` + `pam_sm_setcred`
- `catch_unwind` at the FFI boundary; single `PAM_SUCCESS` construction site
- Remote-session rejection (`PAM_RHOST`, `SSH_*`) before any IPC
- `SO_PEERCRED` check before reading the request body
- Cancellation: socket close → daemon aborts, releases camera, logs as cancelled
  (must not feed the rate limiter)
- **CI assertion: `cargo tree -p faciolock-pam` contains only `libc` + `faciolock-ipc`**

**Exit:** `sudo` authenticates by face on the reference machine. Every failure path
verified to fall through to the password prompt. Daemon killed mid-auth → password
prompt, no hang.

---

## Phase 4 — Storage, TPM, policy (week 5)

- Template store keyed by **uid**, multiple templates per user
- TPM2 sealing via `tpm2-tss`; conservative PCR set; documented re-enrol path
- Non-TPM fallback: root-only file, reduced guarantee documented
- Rate limiter: per-user exponential backoff, **persisted across daemon restarts**
- Lockout: after N failures, face disabled until a successful password auth
- Audit logging to the journal: verdict, reason, liveness score, distance, peer PID/UID

**Exit:** template file copied to another machine is unusable. Kill the daemon
mid-backoff, restart, backoff persists. Journal shows every attempt.

---

## Phase 5 — CLI & enrolment (week 6)

- `faciolockctl enroll` — guided multi-capture, quality feedback, tier detection
- `faciolockctl list` / `remove` / `test` / `diagnose` / `bench`
- polkit policy for privileged operations
- `bench` reports p50/p95/p99, not mean

**Exit:** a person who has not read this wiki can enrol and use it from the CLI alone.

---

## Phase 6 — Integration & hardening (week 7)

- systemd `.socket` + `.service`, socket activation, idle-exit
- Full sandbox profile — target `systemd-analyze security` ≤ 2.0
- Pre-warm session agent on the compositor's idle/lock signal
- All six PAM services: `sudo`, `su`, `kde`, `sddm`, `polkit-1`, `login`
- `cargo-fuzz` on the IPC parser and the frame-handling path

**Exit:** zero RSS when idle (`pidof faciolockd` empty). KDE lock screen unlocks by
face *and* password entry still works instantly while the camera is running.

---

## Phase 7 — Packaging & release (week 8)

- Four binary packages ([07](07-desktop-and-packaging.md#packaging-plan))
- `/usr/share/pam-configs/faciolock`, `Default: no`, `Priority: 512`
- Reproducible builds
- README with the security ceiling stated as plainly as Howdy states theirs
- Hardware compatibility list, seeded with the reference machine
- Documented recovery path if the module misbehaves
- Security policy / CVE contact
- **PPA release**

**Exit:** clean install on a fresh Kubuntu VM, enrol, unlock, uninstall — no manual
`/etc/pam.d` editing at any point.

---

## Beyond v1

- KDE settings module (KCM) — QML/Kirigami, not Rust, separate deliverable
- Debian ITP → archive ([07 checklist](07-desktop-and-packaging.md#the-upstreaming-checklist))
- Independent security review — ideally someone who has reviewed PAM modules before
- GNOME/GDM support
- Hardware compatibility expansion from user reports

---

## Risk register

| Risk | Phase | Mitigation |
|---|---|---|
| Falloff test can't separate face from print | 0 | **Deferred 2026-08-04 by decision.** No print recorded, so this is not yet found at all. Mitigation if it materialises is unchanged — passive texture/reflectance checks, tighten to `ir-strobe`-only — but it would now land later than week 1, when it is more expensive. Accepted. |
| ~~`tract` can't load the models~~ | 0 | **Retired.** `tract` loads YuNet and SFace. Q2 decided on `tract`. |
| ~~EdgeFace weights not BSD-licensed~~ | 0 | **Materialised.** Weights are CC BY-NC-SA 4.0. SFace fallback taken as designed ([B10](11-bug-log.md#b10)). |
| SFace training-data provenance also restricted | 0 | Unresolved. Upstream repo states no licence. Fallback is dlib's CC0 model. **Must clear before ITP.** |
| Embedder is 53% of the latency budget | 1 | SFace is 85 ms under `tract`. Detector input size is the cheap lever; quantized models are **not** available under `tract`. |
| ~~Frame burst is 130 ms estimated, never measured~~ | 2 | **Materialised, worse than forecast.** The delivered frame rate is **12–15 fps, not 30** — 30 was the advertised rate and nothing had measured the real one ([B19](11-bug-log.md#b19)). A 6-frame burst spans ~340–410 ms, which is the whole p95 target before any inference. And the 130 ms was impossible under its own assumption: 6 frames at 33.3 ms span ≥167 ms. The "82 ms of headroom" this row used to quote was itself stale — [06](06-performance-budget.md) has read ~399 ms against a 400 ms target since Phase 1. |
| **Ambient IR breaks every ratio-domain statistic; all corpus data was nocturnal** | 2 | **Materialised.** [B18](11-bug-log.md#b18). `classify` fixed by moving to the difference domain; **`dark_ratio_max` still rejects 100% of honest daylight captures** and its replacement is blocked on a floodlight-in-daylight recording. Best candidate separates by 2.1× where the old check had 65×. |
| TPM PCR choice breaks enrolment on firmware update | 4 | Conservative PCR set; documented re-enrol; non-TPM fallback always available. |
| Reference machine is the only self-strobing hardware | 2 | `ir-constant` tier exists; needs testing on borrowed hardware before any claims. |
| Rust crate packaging blocks the archive | 7+ | Known-slow, not blocking. PPA first regardless. |
| 8 weeks is optimistic | all | It is. Phase exits are the checkpoint, not the calendar. |
