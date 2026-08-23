# 09 — Roadmap

**Scope: full vertical.** Everything in [01](01-goals-and-scope.md) gets built.
Estimate ~8 weeks of steady part-time work to a shippable PPA release; the archive
tail ([07](07-desktop-and-packaging.md)) runs well beyond that.

**Ordering principle:** unproven claims first, known engineering last. Two things in
this project could still be wrong — that `tract` can run our models, and that the
falloff test separates a live face from a flat print. Both are answerable in week 1
for near-zero cost, so they go in week 1. Everything after that is work we already
know how to do.

**Status: both are settled, and the second one came back negative.** `tract` runs both models,
Q2 decided. And on 2026-08-19 a magazine cover finally supplied the print sample S5 had been
waiting for since day one: **the falloff test does not separate a live face from a flat
printed one.** The print reads CV 0.2733 against a worst live face of 0.2661 — above it, so
no threshold exists at any value that admits every live capture and rejects the print
([B24](11-bug-log.md#b24)). A printed face passes every enforced liveness check.

That is the outcome S5 was written to allow ("or evidence that falloff alone is
insufficient"), so Phase 0 exits rather than stalls, but it makes liveness an open design
problem rather than a calibration one. `centre_edge_ratio` looked like the replacement —
prints 0.876–0.918 against live 1.446–1.774 — and the separation is real but far thinner than
it measured: the corpus test takes the worst of 41 pairs where the daemon judges 3, and per
pair the print's best reading is 1.443 against the weakest live capture's worst of 1.446. A
**~0.2% margin**, from one print object and one human subject ([B24](11-bug-log.md#b24)).
**No threshold was taken, and liveness has no check that rejects a print.**

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
- **S5 — Threshold spike.** ~~Compute `D`, ON/OFF ratio, and spatial falloff over the
  corpus. Plot the separation.~~ **Answered 2026-08-19, negatively.** Falloff CV does not
  separate live from print — 0.2733 print against 0.2661 worst live, an interleave rather
  than a margin ([B24](11-bug-log.md#b24)). The magazine-cover recording that settled it took
  eight minutes and had been deferred since 2026-08-04.

**Exit: met, on its second branch.** ~~a number for `tract` vs `ort`~~ **done**; and the
alternative the criterion explicitly allowed — "evidence that falloff alone is insufficient
and passive texture checks are also needed" — is now in hand rather than the plot. The phase
closes with a worse design position than it hoped for and an honest one, which is what an
exit criterion phrased as a disjunction is for.

**What it hands to Phase 1:** a liveness stage that contributes nothing against a printed
face, and one measured candidate (`centre_edge_ratio`) to rebuild it around.

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
   **Met 2026-08-09.** The daemon binds `SOCK_SEQPACKET` at 0600, loads and SHA256-verifies
   both models, keeps a warm stream, and answers. Measured on the reference machine:

   ```text
   startup      6 frames warm in 339 ms          <- matches B19's 340-410 ms
   Op::Status   not enrolled -> NoEnrolment      0 ms, no camera touched
   Op::Status   enrolled     -> Deny             never Allow; one construction site
   Op::Auth     -> Environment{floor 85.44} -> Unavailable      365 ms
   Op::Enroll   -> Unavailable                   no unauthenticated enrolment endpoint
   38-byte datagram (max 37) -> refused, no reply
   ```

   The first run of this criterion found [B22](11-bug-log.md#b22), which would have made
   every live authentication fail. That is what the criterion is for.

   **Floor 85.44 is a new corpus maximum** (previous 51.74), in the operator's room at
   14:59 IST. The `Environment` answer is the documented daylight refusal, not a fault —
   criterion 2 below is what closes it.

If the burst cost is confirmed at ~340 ms, the latency conversation moves to **overlapping
capture with compute** — SFace's 230 ms can hide inside the capture window — which is a
pipeline change and needs its own decision entry rather than a quiet re-open of Q2.

---

## Phase 3 — PAM module & IPC (week 4)

The security-critical week. **Built 2026-08-09**, and taken deliberately out of order:
Phase 2's criterion 2 is blocked on recordings that can only be made after dark, and this
is the one phase that needs no camera at all.

- ~~`faciolock-ipc`: fixed-layout binary protocol, hard size caps~~ — **done in Phase 2**
- `faciolock-pam` cdylib: `pam_sm_authenticate` + `pam_sm_setcred` — **done**
- `catch_unwind` at the FFI boundary; single `PAM_SUCCESS` construction site — **done**,
  and the single site is held by two tests, one of which cross-checks it against
  `faciolock_ipc::Verdict::is_allow` so the module's rule and the daemon's cannot drift
- Remote-session rejection (`PAM_RHOST`, `SSH_*`) before any IPC — **done**
- `SO_PEERCRED` check before reading the request body — **done**, and a refused peer gets
  **no reply at all**, because answering would tell an unprivileged process whether a given
  user is enrolled ([03](03-pam-and-security.md#r2--the-daemon-decides-who-you-are-the-pam-module-states-who-you-claim-to-be))
- Cancellation: `POLLRDHUP` at the stage boundaries, logged as cancelled, no reply — **done**
- **CI assertion: `cargo tree -p faciolock-pam` contains only `libc` + `faciolock-ipc`** —
  **done as a test**; there is no CI in the repository yet to run it

Two things the build settled that the plan did not anticipate:

- **The 200 KB module budget holds, but not on disk.** 353 KB stripped, **164 KB
  resident** — the gap is `std`'s backtrace symbolizer, never touched, never paged in.
  [06](06-performance-budget.md#memory-budget) has the method and why `panic = "abort"`
  stays off the table.
- **libpam is linked, not depended on.** The four functions the module calls are declared
  by hand, so the dependency tree stays at `libc` + `faciolock-ipc` while the `.so` still
  carries a `DT_NEEDED` for `libpam.so.0` ([07](07-desktop-and-packaging.md#packaging-plan)).

Two hang bugs were caught in review before any of this was committed, both of them the
failure mode [03](03-pam-and-security.md) ranks first — an unusable lock screen:

- The client set its socket timeouts **after** `connect`. An `AF_UNIX` `connect` against a
  listener whose backlog is full does not fail, it sleeps; a daemon wedged in a stuck
  camera ioctl stops accepting, eight queued clients fill `BACKLOG`, and the ninth blocks
  with no deadline on it. Fixed by setting the timeouts before `connect`, where
  `SO_SNDTIMEO` bounds the sleep. `a_full_backlog_times_out_instead_of_blocking_forever`
  hangs indefinitely against the old ordering, which is how it was confirmed rather than
  assumed.
- The **daemon** had no read timeout on an accepted connection. It is serial, so one root
  peer that connected and sent nothing parked it forever. Now bounded at 2 s, which is
  wiki/04's "daemon-side hard timeout regardless of what the client asked for" applied to
  the stage it had been missing from.

And [B23](11-bug-log.md#b23) came out of it: the socket's 0600 was being bought with a
*process-global* `umask`, which a second concurrent caller turned into a corrupted
directory somewhere else entirely.

**Exit:** `sudo` authenticates by face on the reference machine. Every failure path
verified to fall through to the password prompt. Daemon killed mid-auth → password
prompt, no hang.

**Status against that criterion.** The failure paths are demonstrated; the success path is
not, and cannot be until there is an enrolment, which needs the camera.

**Observed** means someone ran it and read the output. **Pending** means the check is
written and has not been run. The distinction is [B19](11-bug-log.md#b19)'s lesson and it
is cheap to keep.

| Leg | State |
|---|---|
| Non-root peer → refused by `SO_PEERCRED`, no reply | **observed**, against the real daemon and without root: an unprivileged client sending a well-formed `Op::Auth` gets `ECONNRESET`, and the daemon logs `refused pid 27608 uid 1000 — only root may ask`. The camera is never touched and nothing is written to the socket |
| Client survives every hostile reply — garbage, oversized, truncated, wrong version, silence, a full backlog | **observed**, `cargo test -p faciolock-pam`, against real kernel sockets rather than a mock |
| Module loads under a real libpam, out of a real pam.d file | **observed** — `/etc/pam.d/faciolock-test` + `tools/pam-try.c`, 10 checks, 10 passed |
| Daemon absent → `PAM_AUTHINFO_UNAVAIL` | **observed**, 0.3 ms — `connect` fails, nothing to wait for |
| Not enrolled → `PAM_USER_UNKNOWN` | **observed**, 0.6 ms cold, and the camera is never opened |
| Remote session → refused **before** any IPC | **observed**, 0.2–0.4 ms, and the daemon's log did not grow by a line while it was up and demonstrably reachable — which is the only way to show an absence |
| Daemon gone between requests → no hang | **observed**, 0.4 ms, `ECONNREFUSED`. A kill *during* a request needs a slow request, so it needs an enrolment; the mechanism is covered by the client's EOF and timeout tests |
| Daemon's 2 s read timeout on a silent root peer | **observed** — `pid 28726 uid 0 sent nothing (EAGAIN); closing`, and the next request was answered normally. It cannot be unit-tested: the timeout is set after the `is_root` gate and tests run as uid 1000 |
| **`sudo` authenticates by face** | **not done — needs enrolment (camera) and a separate approval to touch the sudo stack** |

The last row is the phase exit and it stays open. What is left in it is not code.

Every `PAM_AUTHINFO_UNAVAIL` above was matched against the module's own `debug` line in the
journal, so each one is the failure it claims to be rather than a shared code: `No such
file or directory` for the absent daemon, `Connection reset by peer` for the `SO_PEERCRED`
refusal, `refusing a remote session (SSH_CONNECTION)` and `(SSH_TTY)` with no socket
touched, `Connection refused` for the stale socket.

### The harness found a 1.8 s cold-start stall, which is now fixed

Its first authentication took **1753 ms** to answer `NoEnrolment` — an operation that
touches no camera and reads one absent file, and that measured **0.4 ms** on the very next
call. Nothing was looking for this; it came out of a timing column added so the "fast"
claims in the table above would be checkable.

```text
                          before             after
exec -> socket bound      377 ms             377 ms
     -> daemon accepts   2156 ms             378 ms
     wait after connect  1779 ms               0 ms
harness, cold NoEnrolment
       through real PAM  1753 ms             0.6 ms
```

The last row is the same harness re-run after the fix, so the end-to-end path confirms the
socket-level measurement rather than only agreeing with it in principle. The daemon log
settles the ordering by itself: `refused pid 30811 uid 1000` is written *before*
`camera warm`, meaning the first four checks were all answered while the camera was still
warming.

`Daemon::serve` bound the socket, **then** warmed the camera on the same thread, then
accepted. Anything connecting in between was queued in the kernel with nothing to observe —
the same behaviour that made the connect deadline necessary, arriving from the other side.

The warm moved to its own thread behind the `Session` mutex. The socket answers
immediately, only work that needs the camera waits for it, and the daemon log proves the
order by recording a peer refusal *before* the warm finishes. An authentication arriving
mid-warm still waits — there is one camera — but it waits connected, with its deadline
running, rather than unaccepted.

This mattered most for **Phase 6**, which it is now cheaper to build: socket activation
makes cold start the normal first unlock, and the pre-warm agent
([04](04-architecture.md#idle-cost-how-zero-rss-when-idle-works)) only works if a poke can
be answered *while* the camera is warming. Under the old ordering it could not be.
[06](06-performance-budget.md#cold-start--measured-2026-08-09-found-by-the-phase-3-harness-fixed-the-same-day)
carries the numbers.

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
| **Falloff test can't separate face from print** | 0 | **MATERIALISED 2026-08-19, exactly as written.** Deferred on 2026-08-04, found fifteen days and three phases later for the price of one magazine cover: print CV 0.2733 against worst live 0.2661, and a flat printed face passes every enforced liveness check ([B24](11-bug-log.md#b24)). The staged mitigation is now the live work, and the obvious candidate is not yet usable: `centre_edge_ratio` does separate (prints 0.876–0.918 vs live 1.446–1.774) but the margin collapses to ~0.2% at the burst length the daemon actually uses, on n=1 print and n=1 subject. No threshold was added. The cost of the deferral is that every week it sat, the falloff design looked settled. |
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
