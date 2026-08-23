# 06 — Performance Budget

Targets are commitments, not aspirations. Each has a measurement method. If we can't
measure it, we don't claim it.

## Latency budget — warm path (daemon resident, camera pre-warmed)

**Restated 2026-08-04 after Phase 1**, single-threaded, Intel Core Ultra 5 225H, release
build. Two decisions moved this table and they very nearly cancelled:

- **Matching is 2-of-3, not 2-of-2** — a third SFace pass, so a blink or a blur is
  outvoted rather than failing the attempt.
- **The detector runs at 320×320, not 192×192** — measured, not preferred. Detecting at
  192 costs 0.82 embedding cosine against a high-resolution reference, which is worse than
  8 px of landmark error and comparable to the gap between two different captures of the
  same person. [11/B13](11-bug-log.md#b13).

Everything marked **measured** is timed inside `faciolock-vision`'s own test suite
(`cargo test -p faciolock-vision --test pipeline_corpus -- warm_path_stage_latency`),
median of 50 iterations after 10 warm-up.

| Stage | ms | Basis |
|---|---|---|
| `connect()` + request | 1 | estimate |
| ~~Frame burst (6 frames @ 30 fps)~~ | ~~130~~ | ~~estimate~~ |
| **Frame burst (6 frames @ 14.7 fps)** | **404** | **measured** — p50 of 50 trials from a warm streaming session, p95 408, p99 408 |
| Burst build + ON/OFF pairing | **0.3** | measured |
| Liveness, pre-box | **0.6** | measured |
| YuNet detection (×1) @ 320×320 | **33.3** | measured |
| Liveness, post-box | **< 1** | measured |
| Alignment ×3 | **0.8** | measured |
| SFace embedding ×3 | **230.6** | measured, 76.9 ms each |
| Match + policy + response | 2 | estimate |
| **Total** | **~673 ms** | |

**p95 target: ≤ 400 ms. The warm path misses it by 1.7×, and the frame burst alone
exceeds it.** Measured 2026-08-06, and the two things that moved are worth separating:

- **The rate was never 30 fps.** Delivered is 14.7 fps; 30 was the advertised figure and
  nothing had ever timed it ([B19](11-bug-log.md#b19)). Six frames span 340 ms of sensor
  time on that clock.
- **The 130 ms estimate was impossible under its own assumption anyway.** Six frames at
  33.3 ms span ≥167 ms first-to-last. The arithmetic was wrong independently of the
  hardware.

The remaining ~64 ms between the 340 ms sensor span and the 404 ms wall clock is the
queue flush and phase alignment — what P2-c's fresh-frames-only rule costs, and it is
cheap for what it buys.

**What this rules out.** The gap is 273 ms and the only line items big enough to close it
are the burst itself and SFace. Dropping to 2-of-2 matching saves 77 ms; re-opening Q2 for
`ort` saves ~118 ms; together that is 195 ms and still misses. **The burst is sensor-bound
and cannot be optimised at all** — it is the shutter, not the code. So the honest options
are to restate the target, or to overlap capture with compute: SFace's 231 ms can run
inside the 340 ms the sensor is still exposing, which would hide almost all of the
inference. That is a pipeline change and needs its own decision entry.

Two caveats that must not be dropped when quoting any of this:

1. **Summing per-stage p50s does not give a p95.** The measured figures are medians of
   each stage in isolation. A real p95 for the whole path needs `faciolockctl bench`,
   which is Phase 5.
2. **130 ms of the total has still never been measured.** It is the frame burst, it is
   sensor-bound, and it is the term most likely to move under a stalling camera or a
   throttled laptop. Measuring it stays a **Phase 2 exit condition**.

SFace is **58%** of the budget and the frame burst 33%. If it ever misses, the embedder is
the lever: a smaller detector input is now known to cost accuracy rather than buy time
cheaply, so it is not the first thing to reach for.

### The 85 ms → 76.9 ms correction

The previous table carried SFace at 85 ms from the Phase 0 spike. Re-measured on the same
machine it is 76.9 ms, and the difference is not the point — the point is how nearly the
opposite error was made. A first pass at this measurement used 12 iterations with no
warm-up and reported **39 ms**, which would have shown a comfortable budget. The spike
binary re-run in the same session reads `min 38.98 / p50 76.19 / p95 76.68`: the
distribution is bimodal because the CPU boosts for the first few inferences and then
settles, and a 12-sample median lands inside the boost window.

**A benchmark that does not outlast the boost clock is measuring the clock.** Same family
as [B11](11-bug-log.md#b11), where a number was trusted because it agreed with the
expected answer.

**The 130 ms frame burst is the weakest number in this table.** It is an estimate, it has
never been measured, and it is the term most likely to move under a stalling camera or a
throttled laptop. At 82 ms of headroom a 30% miss there puts us at ~360 ms.
**Measuring it is a Phase 2 exit condition** — see [Q2](08-open-questions.md#q2--inference-runtime--decided-tract).

**Every model figure in this table states its input size, deliberately.** The "5.6 ms
YuNet" line this table used to carry was OpenCV's published benchmark at 160×120 on
unknown hardware, and reasoning from it produced a wrong conclusion about the
`tract`/`ort` gap — see [B11](11-bug-log.md#b11). **A latency figure without an input
size and a machine is not a measurement.**

## Latency budget — cold path (daemon not running)

| Stage | Budget |
|---|---|
| systemd socket activation + exec | 30 ms |
| mmap models, runtime init | 60 ms |
| V4L2 open + `STREAMON` + first frame | 200–600 ms ← **hardware-dependent, unbounded in practice** |
| …then the warm path | 318 ms |
| **Total** | **~610–1010 ms** |

Still 2–3× better than Howdy's per-auth cold start, but visible. Hence pre-warm.

## Pre-warm

The user-session agent pokes the socket on the compositor's "about to lock" /
"idle" signal. If it fires ≥ 300 ms before PAM runs, the cold path is entirely
hidden and every authentication is a warm one. Cheap to build, biggest single
perceived-speed win in the project.

Fallback if the compositor gives us no usable signal: keep the daemon alive with a
longer idle timeout on AC power. Worse for the idle-RSS goal — a trade-off to expose
in config, not to decide silently.

## Memory budget

| State | Target | Why |
|---|---|---|
| Idle (no auth in living memory) | **0 B** | process is not running; socket unit only |
| Daemon resident, idle | ≤ 25 MB RSS | models mmap'd, buffers released |
| During a match (SFace) | ≤ 120 MB peak | 37 MB model + runtime arena + frame buffers |
| PAM module, in the root process | ≤ 200 KB | it is a socket client |
| PAM module — **measured 2026-08-09** | **164 KB resident**, 353 KB on disk | built and weighed, Phase 3 |

The PAM module figure is the important one and it is the direct consequence of the
`libc`-only dependency rule in [04](04-architecture.md#crate-layout). Every process
that authenticates a user — including `sudo`, invoked constantly — pays it.

**It holds, and the two numbers disagree for a reason worth writing down.** The built
`pam_faciolock.so` is 353 KB stripped — 76% over the budget if disk size were the metric —
but only **164 KB** of it is ever resident after `dlopen`, which is the number this row was
always about and the one [its measurement method](#how-each-number-gets-measured)
specifies. The gap is `std`'s backtrace symbolizer: `gimli`, `addr2line` and `miniz_oxide`
account for ~200 KB of DWARF-parsing code that runs only when a panic prints a backtrace,
so those pages are never faulted in. Method: `dlopen` the module into a process that has
already loaded libpam, then sum `Rss` over its mappings in `/proc/self/smaps`.

Removing it from the binary is not available on stable Rust: it needs `-Z build-std`, and
the other lever — `panic = "abort"` — is **forbidden here**, because the module's fail-closed
guarantee is a `catch_unwind` at the FFI boundary ([03](03-pam-and-security.md)) and
`abort` would turn a bug in the module into a killed `sudo` instead of a password prompt.
`opt-level = "z"` + fat LTO were measured and returned 4 KB, which does not pay for a
second build profile.

Idle CPU: **0%**, by construction. No polling, no timer, no background thread. This
is what socket activation buys and it is worth more than any micro-optimisation.

## Cold start — measured 2026-08-09, found by the Phase 3 harness, fixed the same day

The harness's first authentication answered `NoEnrolment` — one absent file, no camera — in
**1753 ms**, and the same call took **0.4 ms** immediately after. Measured directly:

```text
                          before             after
exec -> socket bound      377 ms             377 ms      model load + SHA256 of 39 MB
     -> daemon accepts   2156 ms             378 ms
     wait after connect  1779 ms               0 ms
cold NoEnrolment through
       a real PAM stack  1753 ms             0.6 ms      the harness, re-run
```

`Daemon::serve` bound the socket, **then** warmed the camera on the same thread, and only
then accepted. A client connecting in between sat in the kernel's backlog with nothing to
observe — and every request paid it, including the ones that never touch a camera.

The warm now runs on its own thread behind the `Session` mutex. The socket answers
immediately and only work that genuinely needs the camera waits for it; the daemon log
proves the ordering by recording a peer refusal *before* the warm completes. That also
restores the point of the pre-warm agent above: its poke gets answered while the warm is
still running, which is the entire reason for poking early.

What this does **not** change: an authentication arriving mid-warm still waits for the
camera, because there is one camera. It waits on the mutex, where the client is connected
and its deadline is running, instead of unaccepted in a queue.

Still the number to watch at Phase 6, where socket activation makes cold start the normal
first unlock: model load 377 ms + camera warm + the authentication itself.

## Disk / install footprint

| Item | Size |
|---|---|
| `pam_faciolock.so` | **353 KB measured** (was estimated ~300 KB) |
| `faciolockd` | ~25 MB (Rust + `tract` kernels, stripped; measured on the spike binary) |
| `faciolockctl` | ~2 MB |
| YuNet | 85 KB |
| Embedder (SFace) | 37 MB |
| **Total** | **~62 MB (SFace + tract, zero native deps)** |

vs. Howdy's Python + dlib + OpenCV chain, which pulls in hundreds of MB.

## How each number gets measured

| Claim | Method |
|---|---|
| End-to-end latency | `faciolockctl bench` — N runs, report p50/p95/p99, not mean |
| Per-stage breakdown | tracing spans in the daemon, dumped as a flamegraph |
| PAM module RSS | `smem`/`/proc/self/smaps_rollup` diff on a process before and after `dlopen` |
| Daemon peak RSS | `systemd-run --scope -p MemoryMax=` bisection, plus cgroup `memory.peak` |
| Idle cost | `systemctl status` showing the service inactive; `pidof faciolockd` empty |
| FRR | scripted repeated auth attempts by the enrolled user across a session |
| FAR / spoof resistance | the offline corpus replay in CI ([05](05-vision-pipeline.md)) |
| Sandbox quality | `systemd-analyze security faciolock.service` ≤ 2.0 |
| PAM dep-tree purity | `cargo tree -p faciolock-pam` asserted in CI to contain only libc + ipc |

## Anti-goals

- No GPU/NPU in v1. It adds driver dependency hell for ~100 ms we don't need.
- No multithreading in the vision pipeline until profiling says the single-thread
  path is actually the bottleneck. At these model sizes it very likely isn't, and
  thread pools cost startup time — the thing we're optimising.
- No caching of verdicts. Ever. See [03/R3](03-pam-and-security.md#r3--verdicts-are-not-transferable).
