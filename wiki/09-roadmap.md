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
| **`sudo` authenticates by face** | **still not done, but no longer blocked.** Enrolment existed from 2026-08-23 (Phase 5) and a real face returned `Allow` through the daemon that same evening — so what is left is only the second half of this row: a separate approval to edit `/etc/pam.d/sudo`, which has its own recovery story |

The last row is the phase exit and it stays open. What is left in it is not code, and as of
Phase 5 it is not a missing capability either — it is a decision about the sudo stack.

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

## Phase 4 — Storage, policy, audit (week 5)

**Built 2026-08-23.** Renamed from "Storage, TPM, policy": the TPM is deferred, and the
section title should say what the phase contains rather than what it was going to.

- ~~Template store keyed by **uid**, multiple templates per user~~ — **done**, uid-keyed and
  encrypted. One template per uid holding *n* embeddings, which is what
  `faciolock_vision::Template` already was; separate named enrolments (glasses on/off as
  distinct profiles) were dropped as unrequested
- ~~TPM2 sealing via `tpm2-tss`; conservative PCR set~~ — **deferred, decision P4-a**. The
  seam is in place: `store::KeyProvider` is a discriminant on disk and inside the AEAD's
  associated data, so a `Tpm = 1` variant is a new value rather than a format break.
  [Q5](08-open-questions.md#q5--tpm-sealing-v1-or-later-and-which-pcrs) records what would
  re-open it
- **Non-TPM fallback: root-only key file, reduced guarantee documented** — **done**, and it
  is now the *primary* path rather than the fallback
- ~~Rate limiter: per-user exponential backoff, persisted across daemon restarts~~ — **done**
- ~~Lockout: after N failures, face disabled until a successful password auth~~ — **done**,
  released through `pam_sm_acct_mgmt` and a new `Op::Clear` (decision P4-b)
- ~~Audit logging to the journal~~ — **done**, one structured line per attempt with a
  syslog priority prefix and no `libsystemd`

### What the build settled that the plan did not anticipate

**The store's threat model changed under it.** R5's rate limiter was written as a backstop
behind liveness — "what turns *a photo might work* from a 200-attempt certainty into a
3-attempt gamble". [B24](11-bug-log.md#b24) inverted that: a flat printed face clears every
enforced liveness check. Until a check exists that rejects a print, the failure budget is not
defence in depth, it **is** the defence, and `ratelimit.rs` says so in its header rather than
leaving the reader to infer it from a bug entry three files away.

**wiki/04's data layout was internally inconsistent, and this phase is where it mattered.**
It specified `/var/lib/faciolock/users/<uid>.tpl` as `root:root 0600` while R7 runs the
daemon as `User=faciolock`. Both cannot hold — the daemon must read templates to do its job.
Corrected in [04](04-architecture.md#data-layout): the directory, the templates and the key
belong to the daemon's own user. The honest consequence, now written down, is that anything
running as `faciolock` can read the key; that is one process, and Phase 6's sandbox is what
keeps it one.

**`PAM_IGNORE` was verified against the shipped library, not the header.** `libpam0g-dev` is
no longer installed on the reference machine, and `pam.rs`'s standard is that its constants
are *read* rather than recalled. `pam_strerror` out of `libpam.so.0` is the stronger source
and it cross-checked the four constants already there.

### The one rule that is not negotiable in the account stack

`pam_sm_acct_mgmt` returns `PAM_IGNORE` on **every** path — dead daemon, refused socket,
malformed reply, panic. A failure returned from the *auth* stack falls through to a password
prompt; a failure returned from the **account** stack refuses the login outright. Faciolock
has no opinion about whether an account is valid and must never be able to express one. The
`catch_unwind` there discards its result deliberately, and it is the only place in the crate
where that is right.

**Exit:** template file copied to another machine is unusable. Kill the daemon
mid-backoff, restart, backoff persists. Journal shows every attempt.

**Status: met.** The first criterion is closed in CI; the second and third were observed on
2026-08-23 through `tools/ratelimit-try.sh`, 23 checks, 23 passed. The one leg that was still
indirect — a failure the *daemon itself* recorded — closed later the same day through Phase
5's harness, once there was an enrolment to fail against.

**The harness's first run reported 8 failures and every one of them was the harness.** Worth
recording, because two were wrong beliefs about the daemon rather than typos:

- It edited `ratelimit.db` behind a **running** daemon and expected it to notice.
  `Limiter::load` runs once at startup and the state is held in memory, written through on
  change. The daemon was right and the test was wrong — the fix is to write the budget while
  it is down, which is also what the criterion actually describes.
- It waited for the socket **inode** to appear after a restart. `kill -9` leaves the inode
  behind, so the wait returned instantly and the request hit a dead socket. Readiness now
  means a real answer to a real request.

The other four were a **stale binary**: `cargo test` builds test binaries and does not
rebuild `target/debug/faciolockd`, so the run exercised a daemon from before the audit
changes. That is a lesson about the harness's own preconditions, and it is why the script
now checks the daemon answers before it asserts anything about what it said.

| Leg | State |
|---|---|
| A copied template does not decrypt | **observed**, `cargo test -p faciolock-daemon`. There is no second machine here, so "another machine" is modelled as what a second machine *is* to this format — a different key — and the test asserts the two stores really did generate different keys, so the refusal is not passing for some other reason |
| A template cannot be moved between uids | **observed**. The uid is authenticated as associated data, so renaming `1000.tpl` to `0.tpl` fails to decrypt rather than handing over root's enrolment. This is A3's *implantation* half, which the plaintext store failed completely |
| A single flipped bit is an error, not a different face | **observed**, at three offsets |
| Nothing recognisable is left on disk | **observed** — asserted on the raw bytes, because "we called encrypt" is not the same claim |
| The failure budget survives a reload | **observed**, unit test |
| The daemon starts, binds and warms with the new config and store | **observed** — 6 frames in 339 ms, unchanged from Phase 2, and the `SO_PEERCRED` refusal still fires |
| A locked-out user is refused before the camera | **observed 2026-08-23**, `sudo tools/ratelimit-try.sh`, 23/23. The proof of *ordering* is that the same user, on the same daemon, answers `NoEnrolment` the moment the lockout is cleared — so the budget was what answered first, not the store |
| `Op::Clear` releases the lockout through the real socket | **observed**, and it survives a restart. A clear with nothing to clear is silent, which is the property that keeps `acct_mgmt` from writing a journal line on every login |
| The budget survives `kill -9` | **observed, with one leg still indirect.** The file outlives the kill and a restarted daemon refuses on it. That the write lands *before the reply* is shown by `Op::Clear` removing the entry from disk while the daemon is still running — the same `flush()` that `record_failure` calls — rather than by a failure the daemon recorded itself, which needs a camera |
| The journal shows every attempt | **observed** — 5 `auth` lines for 5 requests, 2 `clear` lines for 3 clears, every one carrying uid, verdict, reason, peer, elapsed and `counted=`. Priorities separate: `<3>` lockout, `<6>` not-enrolled |
| `serve::listen` recovers from a socket left behind by `kill -9` | **observed**, and unplanned — the harness needed to know when a restarted daemon was ready, and the stale inode is why the first version of it reported four false failures |
| **A denied face moves the counter** | **observed 2026-08-23**, by Phase 5's harness rather than this one — it needed an enrolment, which needed `faciolockctl`. Three bursts of a face that was not the enrolled one read `Deny, Deny, Deny`, `ratelimit.db` reached `1000 3 …`, and every one was logged `counted=1`. The exemption held at the same time: **nothing that never saw a face was counted** |

That last row was the honest gap for three months of phases, and it was the same gap Phase 3's
exit has — the failure paths are demonstrable without a camera and the success path is not.
Phase 5 built the thing that closes it, which is why `tools/enrol-try.sh` and not
`tools/ratelimit-try.sh` is where it finally got measured.

---

## Phase 5 — CLI & enrolment (week 6)

**Built 2026-08-23.** This is the phase that finally makes an enrolment possible, which is
why Phases 3 and 4 have had the same row open at the bottom of both their tables.

- ~~`faciolockctl enroll` — guided multi-capture, quality feedback, tier detection~~ —
  **done**, three poses, one `Op::Enroll` per pose, accumulated in the daemon (P5-d)
- ~~`faciolockctl list` / `remove` / `test` / `diagnose` / `bench`~~ — **done**
- ~~polkit policy for privileged operations~~ — **done**, `data/polkit/org.faciolock.policy`
- ~~`bench` reports p50/p95/p99, not mean~~ — **done**, nearest-rank, so every number printed
  is a sample that was measured rather than one interpolated between two that were

**And [Q4 is closed](08-open-questions.md#q4--ipc-raw-unix-socket-d-bus-or-both--decided-2026-08-23-raw-socket-only),
against its own recommendation** (P5-a). Q4 had recommended the hybrid — raw socket for PAM,
D-Bus for the CLI — since 2026-08-03, and Phase 5 is the phase that had to spend that rather
than describe it. What the hybrid buys concretely is richer enrolment feedback; what it costs
is `zbus` plus an async runtime inside the process that parses V4L2 buffers. The verdicts
already on the wire turned out to carry the distinction a person acts on — `Deny` is "it
looked and refused", `Unavailable` is "it could not judge" — which is "move closer" against
"close the blinds" with no new protocol.

### What the build settled that the plan did not anticipate

**`Op::Enroll` needed no protocol change at all, and that is a decision rather than luck.**
The obvious design is a response that says how many poses remain. `Response` is four fixed
bytes parsed by a module loaded into `sudo`, `login` and `sddm`; enrolment is the least
security-sensitive feature in the project and that parser is the most security-sensitive code
in it. So the count lives in `faciolock_ipc::ENROL_POSES` and **both ends count** — the CLI
prompts that many times, the daemon finalises on that number, and two constants that must
agree are one constant.

**`list` is the reason P4-d needed correcting.** P4-d said the CLI reaches the store "through
the socket like everything else". A list of uids is a variable-length answer, so honouring
that would have meant growing the same parser for the same bad reason. The CLI now reads the
template *directory* and never its contents (P5-b): a filename is a uid, a mtime is a date,
`remove` is an unlink, and reaching any of it already requires root. What P4-d actually
protects — one consumer for the store code, one implementation of the format — is untouched.

**`diagnose` can only assert a tier upward, and the reason is B18 rather than caution.**
The first instinct is a probe that writes `ir-constant` when it sees no strobe. But "there is
no strobe here" and "somebody is aiming an infrared floodlight at this" are the *same
measurement* — and [B18](11-bug-log.md#b18) established that ordinary daylight produces it
without an attacker. A helpful auto-demotion would hand back exactly the attack P1-e exists
to prevent. So `diagnose` reports `ir-strobe` on a clean strobe and writes nothing otherwise;
`ir-constant` needs a human to type it. `--write` also refuses to overwrite an existing
config, so a diagnostic run in a bright room cannot rewrite a working machine (P5-e).

**The daylight run produced a measurement worth keeping.** Run at 15:21 IST in the operator's
room, `diagnose` read **alternation 1.00 at a bright/dark ratio of 3.5:1**, against wiki/10's
nocturnal 14,000:1 and 100,000:1. The dark frames read ~21 where they read ~0.004 at night.
That is B18 in one line — and it is also why the classifier gates on the *pattern* rather
than the ratio: **the alternation survives ambient infrared intact and the ratio does not**.
[13](13-daylight.md#the-strobe-pattern-survives-daylight-the-ratio-does-not) carries it.

**`faciolockd --enroll` is deleted.** It existed to make `Op::Auth` testable before there was
any way to enrol and its own comment said so. Keeping it would have meant two capture-and-
store paths to hold in sync, one of which nobody runs.

### Two defects caught in review before any of this was committed

Both are the shape Phase 3's review found and both fail in the direction that matters — the
tool reports success and the user discovers otherwise at a lock screen.

- **The enrolment expiry was a total, and it had to be a gap.** `Pending` timed out 60 s
  after the enrolment *began*. Two poses accepted, a minute of fumbling for glasses, and the
  third arrives to an expired `Pending` — so the daemon starts over, answers `Ack` because
  one pose is not three, the CLI counts its third `Ack` and prints "Enrolled", **and no
  template was ever written.** Measured from the last pose instead, "abandoned" means what it
  sounds like. `a_slow_enrolment_survives_as_long_as_poses_keep_arriving` fails against the
  old field, which is how it was confirmed rather than assumed.
- **`faciolockctl enroll` now asks the daemon instead of trusting its own count.** Three
  `Ack`s mean three poses were captured; they do not mean a template exists — a store write
  that failed on the last pose ends the same way. One `Op::Status` after the last pose costs
  nothing, touches no camera, and turns a silent lie into a message naming both causes.

And a third, which is a CLI bug rather than a daemon one: **`enroll` retried forever.** In
daylight every pose answers `Unavailable`, so the loop printed advice and asked again with no
exit but Ctrl-C — which would have killed `enrol-try.sh` through its trap and meant the
carefully written B18 explanation in that harness could never print. A refused *pose* is
still retried indefinitely, because the person is standing right there and moving is the fix;
a capture the daemon **could not judge** now gives up after three, because neither a bright
room nor a busy camera is fixed by pressing Enter again.

**Exit:** a person who has not read this wiki can enrol and use it from the CLI alone.

**Status: met 2026-08-23**, `sudo tools/enrol-try.sh`, **17 checks, 17 passed**, first run, no
harness bugs — which is the first time that has happened and is worth saying because the
Phase 4 harness needed three attempts to be right about its own preconditions.

The run closed Phase 4's last row in the same pass, as expected: the two have been the same
gap since 2026-08-09. A person enrolled from three prompts, authenticated, was denied three
times by a face that was not theirs, and watched the counter reach 3 on disk.

| Leg | State |
|---|---|
| 189 tests across the workspace, no camera attached | **observed**, `cargo test --workspace`, 0 failed |
| The new crate did not leak into the PAM module's tree | **observed** — `cargo tree -p faciolock-pam` still reads exactly `libc` + `faciolock-ipc`, and its CI assertion still passes. This is the rule wiki/04 says a distro reviewer checks first, and a fourth binary in the workspace is exactly the kind of change that breaks it quietly |
| `diagnose` finds the IR camera on real hardware | **observed** — names `/dev/video2`, rejects the RGB node for not offering greyscale and both UVC metadata nodes for not being capture devices. wiki/10 Probe 4's disambiguation, reproduced from our own code rather than from `v4l2-ctl` |
| `diagnose` reads the strobe | **observed twice, at both ends of the ambient range.** Alternation **1.00 in both**; ratio 3.5:1 in daylight and **89,857:1** after dark, on the same machine the same evening. The pair is the evidence for the claim above, not just an illustration of it |
| `diagnose --write` never overwrites | **observed**, unit test, on the raw file contents after a second call |
| An unknown `tier` stops the daemon rather than defaulting | **observed** — `tier = "rgb-ish"` refuses before the socket is bound |
| The daemon starts, binds and warms with the new config | **observed** — 6 frames in 339 ms, unchanged since Phase 2 |
| A pose only joins an enrolment it belongs to | **observed**, unit test, both ways of getting it wrong — another user's uid, and an expired session |
| No enrolment failure can read as a success | **observed**, unit test: every refusal maps to `Deny` or `Unavailable`, and specifically never to `Ack`, which is what the CLI counts as a captured pose |
| **A person enrols and authenticates from the CLI alone** | **observed 2026-08-23.** Three prompted poses, template written (4,676 bytes, `FLT2`, exactly three captures logged), `list` showed it, and the operator's own face returned `Allow` — the **first `Allow` this project has ever produced**, logged at `<5>` and counted |
| **A denied face moves the counter** (Phase 4's last row) | **observed 2026-08-23**, and it closes a row open since Phase 3. Three bursts of a face that was not the enrolled one: `Deny`, `Deny`, `Deny`, `ratelimit.db` reading `1000 3 …`, all three logged `counted=1` — and **nothing that never saw a face was counted**, so P2-g's exemption held while the budget moved |
| Enrolment removes cleanly | **observed** — `remove` unlinked the template *and* cleared the budget through `Op::Clear`, and `Status` went back to `NoEnrolment` |
| `bench` | **not observed.** The one subcommand the harness does not exercise. It is also the one that wants a **release** daemon: an enrolment pose cost 6.6–6.7 s in this debug build against wiki/06's 85 ms for SFace, so any number `bench` prints from `target/debug` is a measurement of `tract` compiled without optimisation and nothing else |

**Not in this phase, and it needs its own decision:** Phase 3's exit — `sudo` authenticates
by face — becomes *reachable* the moment an enrolment exists. It stays open. Editing
`/etc/pam.d/sudo` is a change with its own recovery story and the roadmap has always said it
takes a separate approval.

---

## Phase 6 — Integration & hardening (week 7)

**Started 2026-08-29.** Two decisions taken with the operator before any code, and both go
against the plan as it was written above.

- ~~systemd `.socket` + `.service`, socket activation, idle-exit~~ — **done**.
  `serve::inherited` is `sd_listen_fds` hand-rolled, no `libsystemd`; idle-exit is a `poll`
  timeout honoured only when the socket was inherited
- ~~Full sandbox profile — target `systemd-analyze security` ≤ 2.0~~ — **done, reads 0.6**
  ([03 R7](03-pam-and-security.md#as-shipped--measured-2026-08-29))
- ~~Pre-warm session agent on the compositor's idle/lock signal~~ — **deferred (P6-a)**, and
  not for cost. The design does not work: see below
- All six PAM services — **shipped as one `pam-auth-update` profile, not six files** (P6-b);
  the rollout and its recovery path are in [07](07-desktop-and-packaging.md#rolling-it-out-and-getting-back-out)
- ~~`cargo-fuzz` on the IPC parser and the frame-handling path~~ — **targets written**, plus
  a stable-toolchain half that actually runs here

### The pre-warm agent was specified against a signal that does not exist (P6-a)

Two problems. The first is fixable and the second is not.

It cannot reach the socket — 0600 root:root, so a user-session agent's `connect()` is `EACCES`
before anything is queued. And it pokes at the wrong end of the absence: the plan warmed on
the **lock** signal, but you lock the screen when you *leave* and the daemon idle-exits 30 s
later, so the warm has expired long before you come back. The moment worth warming is "a
person just returned", and on KDE the greeter takes the input that signals it and then calls
PAM itself with nothing observable in between.

What the agent was meant to hide is **~800 ms** — 377 ms of model load plus ~400 ms of camera
warm — and it stays visible. `idle_timeout_secs` is the only honest lever and it is now
config. [04](04-architecture.md#the-pre-warm-agent-does-not-work-as-specified-and-it-is-deferred-p6-a)
carries the breakdown.

### `cargo-fuzz` needs nightly, and this machine does not have it

The `fuzz/` crate is written and excluded from the workspace — `libfuzzer-sys` must never
enter the lockfile that `faciolock-pam`'s dependency-tree assertion rests on, which is the
same reasoning that keeps `spikes/` out and one degree sharper, since the crate it fuzzes
hardest is the response parser inside `pam_faciolock.so`.

But the reference machine runs the distribution's stable Rust, so those targets have never
executed. A fuzz target nobody can run is a phase item that reads as done and has run zero
times, so the same properties are also asserted by seeded generators in `cargo test`:
`faciolock-ipc/tests/hostile_bytes.rs` and `faciolock-vision/tests/hostile_frames.rs`. What
they give up is the coverage-guided search; what they keep is the property.

**Both of them failed their own first run, and both failures were the generator rather than
the parser.** The response test asserts a floor on how many inputs the decoder actually
accepted, and purely random bytes reached it 21 times in 200,000 — a version byte is 1 in 256
before a legal verdict code is 1 in 43. The frame test asserts a floor on how many corrupted
bursts still paired, and a uniformly random truncation landed on a frame boundary 1 byte in
64. Without those two assertions both tests would have passed convincingly while testing the
early returns. Mutation from valid inputs fixed both.

### The lock screen cannot reach the socket either (Q10)

Measured 2026-08-31, before the leg was attempted, and it is P6-a's wall from the other side.
`kscreenlocker_greet` is not setuid, links `libpam` directly and runs inside the user's
session; Plasma 6.6.5 ships no `kcheckpass`, no system D-Bus service and no polkit action
behind it — the password prompt works because `unix_chkpwd` is setgid `shadow`. The socket is
`0600 root:root` with a `SO_PEERCRED` root check behind it, so the module gets `EACCES`,
returns `PAM_AUTHINFO_UNAVAIL`, and the lock screen falls through to the password field
silently and permanently. Nothing breaks; nothing ever shows a face.

`sudo`, `su`, `login` and `sddm` authenticate as root and are untouched by this.

**Decided the same day — [P6-d](README.md#decisions-taken), open the socket.** `0660
root:faciolock-auth`, and R2 moves off the file mode onto `SO_PEERCRED`: root asks about
anybody, anybody else asks about themselves and only `Auth`/`Status`. `Clear` stays root-only
because a session process that can reset its own failure counter has unlimited face attempts,
and `Enroll` stays root-only because it writes a template. Four files and a pure function; the
trade the operator took is that anything running as you can turn the emitter on and spend your
budget, which is where `fprintd` already sits.

It carries one unfixed knock-on — the lock screen cannot clear a spent budget either, so three
refused faces there turn face unlock off until a `sudo` or a `login` clears it. Written up
with the two candidate fixes at
[Q10](08-open-questions.md#the-knock-on-and-it-is-not-fixed-the-lock-screen-cannot-clear-a-spent-budget).

### A daemon that cannot start must still answer the doorbell (P6-f)

The last decision of the phase. Under socket activation the *connection* is what starts the
service, so a daemon that refuses to start — bad tier, a model that fails its SHA256 after an
upgrade, an unparseable config — leaves that connection queued when it exits. systemd sees a
queued connection and starts it again. Same failure, same connection, until `StartLimitBurst`
fails the socket unit; face auth is then dead until somebody runs `systemctl reset-failed`,
and the journal says `start-limit-hit` rather than what actually broke.

`FlushPending=yes` on the socket unit is one line and would end it, but it is a property of
the *socket*, and the socket cannot tell "the daemon is broken" from "the daemon just took its
scheduled idle-exit". It would also drop the honest connection that races an idle-exit, which
today the next activation serves without anybody noticing.

**The daemon can tell the difference, so the drain lives there.** `serve::inherited()` moved
ahead of `start()` — the failure path needs a socket to answer on — and a failed start accepts
and closes whatever is waiting. Closing rather than replying is deliberate: the module reads
EOF and a `Verdict::Unavailable` as the same `PAM_AUTHINFO_UNAVAIL`, and one of the two is
free.

Measured through `systemd-socket-activate` against a config pointing at a models directory
that does not exist:

```text
client: got 0 bytes after 6 ms  -> EOF, connection closed
faciolockd: closed 1 waiting request(s) before exiting; they fall through to a password
faciolockd: …/no-such-models/MANIFEST.sha256: No such file or directory (os error 2)
```

Six milliseconds to a password prompt, rather than waiting out the module's 3 s deadline —
the fall-through got *faster* as well as quieter.

### Enabling the profile turned the camera into a cron job (P6-e)

The first thing the `pam-auth-update` profile did, one day in, was light the IR emitter three
times an hour on an idle machine. The account half of the profile reaches `/etc/pam.d/cron`,
every cron job therefore sends an `Op::Clear`, a connect under socket activation is a service
start, and the daemon warmed the camera before it read the request — so `run-parts` at :17
turned the lens on for a full 30 s idle window. [B29](11-bug-log.md#b29) has the journal.

The startup warm is deleted rather than made conditional. It was written for P6-a's pre-warm,
which does not exist; PAM runs auth before account, so the warm can never precede the `Auth`
it would have helped; and on a cold authentication it *costs* a burst, because the arriving
request waits on the session mutex behind it. `tools/activation-try.sh` PART 2 now asserts the
daemon holds zero `/dev/video*` fds after a request that does not need the camera.

**Exit:** zero RSS when idle (`pidof faciolockd` empty). KDE lock screen unlocks by
face *and* password entry still works instantly while the camera is running.

**Status: met, 2026-08-31 19:20:33.** Zero RSS when idle has held since 2026-08-29; the KDE
lock screen unlocked by face on a trackpad touch, twice, with no keystroke. One leg of the
phase is still open — a face that is *looked at and refused* falling through to the password
prompt has never been watched end to end — plus the `FlushPending=` decision below.

| Leg | State |
|---|---|
| Tests across the workspace, no camera attached | **observed** — `cargo test --workspace`, **198 passed, 0 failed**, 5 ignored (they want hardware). 189 before this phase |
| The fuzz crate did not leak into the PAM module's tree | **observed** — `cargo tree -p faciolock-pam` still reads exactly `libc` + `faciolock-ipc` |
| A `SOCK_STREAM` listener is refused rather than accepted quietly | **observed**, unit test. One word in a unit file (`ListenStream=` for `ListenSequentialPacket=`) produces a daemon that starts, logs "socket-activated" and mis-parses every request |
| An idle daemon exits and leaves no process behind | **observed 2026-08-29**, `systemd-socket-activate` against the shipped binary: `socket-activated`, `camera warm … 6 frames in 339 ms`, `idle for 3 s, exiting`, `pgrep -x faciolockd` empty |
| The socket outlives the exit | **observed**, same run. This is the bug that would have made the *first* idle-exit permanently break reactivation |
| **Reactivation on the same inode** | **observed 2026-08-30**, under real systemd after `install-local.sh`, and it took the install to show it. Two idle-exits, two reactivations on the same inode, from the journal: `idle for 30 s, exiting` → `Deactivated successfully` → `Started faciolock.service` → `camera warm … 6 frames in 339 ms`, twice inside two minutes. `systemd-socket-activate` could not show this: it `exec()`s the daemon rather than supervising it — measured, the activator's pid *is* the daemon's pid — so when the daemon exits nothing holds the listening fd and the next request gets `ECONNREFUSED`. `tools/activation-try.sh` PART 4 starts a fresh activator instead, which shows an exited daemon is replaceable and not that systemd replaces it |
| `systemd-analyze security` ≤ 2.0 | **observed — 0.6**, offline against the unit file |
| The sandbox runs the real workload, not just the scorer | **observed 2026-08-30**, same journal. `User=faciolock` under the full profile opened `/dev/video2` through `SupplementaryGroups=video` + `DeviceAllow=char-video4linux`, read and SHA256-verified both models out of `/usr/share/faciolock/models` under `ProtectSystem=strict`, and warmed in **335–339 ms** — unchanged from the unsandboxed daemon since Phase 2. Peak RSS 106.4 MB, 449 ms CPU over a 45 s life. A 0.6 score on a service that cannot do its job would have been worth nothing |
| `faciolockctl diagnose` against the installed system | **observed 2026-08-30**, and only after it found two bugs of its own — [B25](11-bug-log.md#b25) and [B26](11-bug-log.md#b26), both introduced by this phase, both hit by the operator running this one command on a fresh install. Now: names `/dev/video2`, rejects the RGB node and both metadata nodes, reads **alternation 1.00 at 44.8:1**, and writes `device` + `tier` into `/etc/faciolock/faciolockd.toml` |
| An enrolment in flight blocks the idle-exit | **observed 2026-08-31, after dark.** PART 5 had skipped in daylight — the pose came back `Unavailable`, so there was no `Pending` for the guard to protect and the harness refused to assert on it rather than assert the wrong thing ([B18](11-bug-log.md#b18)). Re-run in the dark: `one pose lands  Ack`, then `the daemon stayed up for the rest of it  yes`. That is [B27](11-bug-log.md#b27)'s fix and `may_exit` both, doing their job against a real half-finished enrolment rather than a unit test |
| `tools/activation-try.sh` end to end, as root | **observed 2026-08-31 — 18 passed, 0 failed, every part.** The daylight run got 15/15 with PART 5 skipped; the after-dark re-run took all five parts, and it is the first time the harness has run start to finish. Nothing behind the socket, the first request starts it, `socket-activated` with no `self-bound`, **the camera untouched by a request that does not need it** ([B29](11-bug-log.md#b29)), idle-exit armed and taken, the socket outliving the exit, a second activation counted as two rather than one restart, and a pose in flight holding the daemon up |
| A real install: units, `faciolock` user, sandbox, module in `/usr/lib/*/security` | **observed 2026-08-30 00:45**, `tools/install-local.sh`. `faciolock` uid 974 from sysusers, `/usr/libexec/faciolock/faciolockd` stripped to 26 MB, `pam_faciolock.so` in `/usr/lib/x86_64-linux-gnu/security`, `/usr/share/pam-configs/faciolock` present at `Default: no`, socket unit enabled. The install deliberately does not touch `/etc/pam.d`, and `/etc/pam.d/sudo` was confirmed unchanged afterwards — installing the module is not enabling it |
| **KDE lock screen unlocks by face** | **OBSERVED 2026-08-31, 19:20:33 — the exit criterion of this phase and the thing the whole project is for.** A trackpad touch woke the greeter and the screen unlocked with no keystroke: `auth uid=1000 verdict=Allow reason=match similarity=0.873 agreeing=3 live=pass probes=3 pid 21264 uid 1000 elapsed=2073`. Twice, `0.873` and `0.881`. The `uid 1000` on that line is [P6-d](README.md#decisions-taken) working — `kscreenlocker_greet` is an unprivileged session process asking about itself through the group gate, which two days earlier got `EACCES` and fell through to the password field forever. **And the greeter does not wait to be typed at**, unlike SDDM: KDE runs `kde`, `kde-fingerprint` and `kde-smartcard` concurrently from the moment the lock screen wakes, so `common-auth` — and our module in it — runs on the *wake*. Only `/usr/lib/pam.d/kde` includes `common-auth`, so we run once per unlock, not once per stack. Cold cost, daemon idle-exited: ~2.5 s from touch to desktop |
| The group gate refuses what it should | **observed**, unit test (`a_session_peer_may_ask_about_itself_and_nothing_else`): root reaches any uid, uid 1000 reaches neither 1001 nor root for any op, an unresolvable name answers nobody but root, and uid 1000 asking about **itself** gets `Auth`/`Status` but not `Clear`/`Enroll` |
| **An unprivileged peer is answered on real hardware** | **observed 2026-08-31.** Reinstall created `faciolock-auth` (gid 973) from sysusers and the socket came back `srw-rw---- root faciolock-auth`; `sock-try status $USER` as uid 1000 — no `sudo`, no PAM — answered **`Deny (0) 4 ms`**, where the same call before this phase was closed on without a reply. That is the whole of P6-d working: a session process asked about itself and got an answer |
| The gate's *refusals* on real hardware | **observed 2026-08-31.** Both directions from the same uid-1000 shell that had just been answered about itself: `status root` and `clear $USER` each printed `no reply (EOF)`, and the daemon logged `refused pid 11960 uid 1000 — Status for "root"` and `refused pid 11966 uid 1000 — Clear for "csr"`. The second is the one that matters — asking about *yourself* is not enough, the op has to be one that cannot move the budget |
| **`sudo` authenticates by face** (Phase 3's exit) | **observed 2026-08-31, and it closes a row open since Phase 3.** Two lines above `@include common-auth` / `@include common-account` in `/etc/pam.d/sudo`, a root shell held in a second terminal, `sudo -K` to kill the timestamp — and `sudo true` returned **with no password prompt at all**. First `sudo` this project has ever authenticated by face. The file was restored from `sudo.faciolock-backup` afterwards, so the trial is over rather than living |
| **A face the daemon cannot judge still lets you in with a password** | **observed 2026-08-31, through `pam-auth-update` rather than the by-hand file.** `sudo -K; sudo true` at a covered lens: a password prompt, and the journal names the path — `auth uid=1000 verdict=Unavailable reason=ambient floor=1.31 live=pass probes=0 elapsed=1782 counted=0`. `default=ignore` watched end to end for the first time instead of argued from Phase 3's table. **`counted=0` is the second half of it**: nothing that never saw a face moved the budget, so P2-g's exemption held under a real `sudo` |
| **A face that is looked at and *refused* still lets you in with a password** | **still not observed.** The run above went `Unavailable`, not `Deny` — the question was never asked, so this is the neighbouring path rather than this one. `default=ignore` treats them identically and Phase 3 observed the return code, but the `Deny` fall-through has never been watched end to end. It needs a face the camera can judge and a face that is not the enrolled one — after dark, alongside the other two |
| Enabling it does not break the password path for every other service | **observed** — `pam-auth-update --enable faciolock` generated `auth [success=2 default=ignore] pam_faciolock.so` above `pam_unix`, and the arithmetic is right: a face success jumps to `pam_permit`, anything else falls to `pam_unix`, whose own `success=1` lands on the same `pam_permit`. It also rewrote `pam_unix` to `try_first_pass`, which is benign **only because our module never runs a conversation** — it stashes no password, so `try_first_pass` finds none and prompts. A future version that ever prompted would silently change `pam_unix`'s behaviour |
| **A broken daemon does not flap the socket unit** | **observed 2026-08-31** ([P6-f](README.md#decisions-taken)). Under `systemd-socket-activate` with `models` pointing at a directory that does not exist: the queued request was accepted and closed on the way out, the client saw **EOF in 6 ms**, and the journal names the real cause instead of `start-limit-hit`. Held by a unit test that queues two clients on a real listener, drains, and asserts both see the connection go, the second drain finds nothing, and the socket reads idle — the test fails if `drain` is stubbed out |
| **`faciolockctl bench` against the release daemon** | **observed 2026-08-31, and it closes a caveat open since Phase 1.** 20/20 allowed, **p50 540 ms, p95 543, p99 556, min 522, max 556** — 34 ms of spread across the whole run. The daemon's own audit log gives p50 540 for the same samples, so IPC and the peer check cost under a millisecond. [B28](11-bug-log.md#b28)'s warm-up held: the 590 ms first sample and the `sudo`'s own cold 1,981 ms are both outside the distribution. **It also contradicts [06](06-performance-budget.md#the-first-end-to-end-measurement-and-it-does-not-agree-with-the-table)'s stage table by 133 ms in the fast direction, which is arithmetically impossible** — 6 frames at 14.7 fps is 340 ms of sensor time alone — so one of the two is measuring something other than its label. Open, and neither number is quotable until the burst is timed inside the daemon |
| **A face login through a display manager** | **observed 2026-08-31 — the first one this project has done.** SDDM, `similarity=0.840`, and the face check and `session opened for user csr` share pid 17672, so they are one `pam_authenticate()`. What it also showed is that the *moment* is the greeter's: SDDM called PAM 10 s after the login screen appeared, when the password was submitted, so the face ran after the typing rather than instead of it — and `[success=2]` then jumped over `pam_unix`, meaning **the submitted password was never checked**. Not our stack order, not fixable from a module: [07](07-desktop-and-packaging.md#when-the-face-check-runs-is-the-greeters-decision-not-ours) |
| **The camera is not touched by an activation that does not need it** | **observed 2026-08-31, on the patched binary and against the machine's own journal.** [B29](11-bug-log.md#b29): with the profile enabled, cron's account stack activated the daemon three times an hour and each activation warmed the camera for a full idle window — `17:09:01`, `17:17:01`, `17:30:01`, each followed by `camera warm … 336 ms`. With the warm deleted, the daemon was started self-bound on a scratch socket, left listening for 2 s and its `/proc/PID/fd` read: **one socket, `/dev/null`, the log, and no `/dev/video*` at all**, where the old binary opened the device 1–2 s after `Started` (model load first) and then held it for the whole idle window. `activation-try.sh` PART 2 now asserts the same count under real activation; that run needs root and the installed socket stopped. **Then confirmed on the installed daemon**: reinstalled 18:39, six activations since, zero `camera warm` lines, including the `19:09:01` cron tick that had lit the emitter at `17:09:01` |
| **The daylight tax is real and it is now on every `sudo`** | **observed** — the refusal above cost **1,782 ms in the daemon** before the password prompt, on top of the model load when the daemon had idle-exited. In a bright room [B18](11-bug-log.md#b18) means face auth *always* refuses, so enabling the profile in daylight buys nothing and costs two to three seconds per `sudo`. Not a defect; it is `idle_timeout_secs` and B18 meeting, and it is the strongest argument yet for the daylight work rather than for a bigger timeout |

---

## Phase 7 — Packaging & release (week 8)

**Started 2026-08-31**, the same day Phase 6 closed. One decision taken before any file was
written, because it blocked every other one.

- ~~Four binary packages~~ — **three** ([07](07-desktop-and-packaging.md#packaging-plan)).
  `plasma-faciolock-kcm` is marked "(later)" in the same table that lists it and is not
  part of v1
- `/usr/share/pam-configs/faciolock`, `Default: no`, `Priority: 512` — **already correct
  since Phase 6**; this phase only ships it
- Reproducible builds — `SOURCE_DATE_EPOCH` from the changelog, `--locked`, vendored
  sources. Not yet verified by building twice
- README with the security ceiling stated as plainly as Howdy states theirs — **done**,
  and it grew two entries rather than losing any
- Hardware compatibility list, seeded with the reference machine —
  **[15](15-hardware-compatibility.md)**, one row
- Documented recovery path if the module misbehaves — **done**, as
  `libpam-faciolock`'s `README.Debian`, so it arrives with the package rather than living
  in a wiki nobody installs
- Security policy / CVE contact — **`SECURITY.md`**, with an out-of-scope section that
  names B24 and B18 as known limitations rather than waiting to be told
- **PPA release** — not started. Needs a Launchpad account and a signing key, which is the
  operator's to hold

### The project had no licence, and that had to be settled first (P7-a)

`README.md` said "Not yet chosen — until a `LICENSE` file exists, no licence is granted",
and it had said so since Phase 0. That is a defensible position for a repository nobody can
install and an impossible one for a package: `debian/copyright` has nowhere to start, and
uploading unlicensed source to a PPA is not a thing you may do.

**GPL-3.0-or-later**, decided with the operator. The argument that carried it is that this
is a system authentication component rather than a library: a vendor shipping a modified
`faciolockd` on a laptop line should have to publish what they changed, and a silently
forked biometric daemon is a worse outcome than the friction GPL-3 adds. Nothing in the
tree objects — all 156 vendored crates are MIT/Apache-2.0 or another permissive licence,
including the one MPL-2.0 and the two BSD-3-Clause, and MPL-2.0 is explicitly
GPL-compatible.

### Vendoring, and why not `dh-cargo` (P7-b)

Launchpad's build machines have no network. Every crate must therefore be *inside* the
source package before it is uploaded, and there are two ways to arrange that:

| | `dh-cargo` | Vendored tree |
|---|---|---|
| What it needs | a `librust-*-dev` source package for each of 156 crates | one `cargo vendor` run |
| Where it leads | the Debian archive | a PPA today |
| Cost | months of packaging other people's crates | a ~194 MB source package |

[Q6](08-open-questions.md#q6--target-debian-archive-or-ppa-and-see--decided-archive) picked
the archive as the *goal* and the risk register has said "PPA first regardless" since it was
written. So the vendored tree, and `tools/mk-source-package.sh` is what assembles it: copy
what git tracks plus the gitignored models, `cargo vendor` into the copy, build there.
`vendor/` never enters the repository, and `fuzz/` and `spikes/` stay out of the tarball
for free by not being tracked as workspace source — which matters more than it sounds,
because `libfuzzer-sys` reaching the lockfile would break the dependency-tree assertion
that Phase 3 rests on.

`3.0 (native)` rather than `3.0 (quilt)`: there is no upstream/downstream split to model
here, and a native package is one tarball instead of three files to keep in step.

### The models are gitignored, and the tarball needs them

38 MB of weights that `.gitignore` has excluded since Phase 0, and `faciolock-models`
cannot be built without them. `mk-source-package.sh` copies them in explicitly rather than
the repository changing its mind about tracking binaries.

**Exit:** ~~clean install on a fresh Kubuntu VM~~ — **on the reference machine, at the
operator's instruction (2026-08-31).** Install, enrol, unlock, uninstall, with no manual
`/etc/pam.d` editing at any point.

What the VM was for was a machine with no history: no `faciolock` user, no
`/var/lib/faciolock`, no group membership, nothing the hand-install left behind. This
machine has all four, so **the install half is a weaker test than the exit criterion asked
for** — a first-install failure to create the user could not show up here, because the user
already existed. That is exactly the failure [B31](11-bug-log.md#b31) would have been, which
is why it was worth catching by reading the generated postinst rather than by running an
install. The removal half is unaffected and is the stronger half of the two.

**Status: exit met, 2026-08-31 23:02:00.** Install → enrol → **unlock** → uninstall, all four
legs on the reference machine, with no `/etc/pam.d` edited by hand at any point: the profile
was enabled by `pam-auth-update` and un-enabled by the `prerm`. The three packages build,
both `lintian` runs are clean, and packaging found five defects that nothing else in this
project could have found — three before any install, one by the purge, and the silent one is
the one worth reading ([B30](11-bug-log.md#b30), [B31](11-bug-log.md#b31)).

**What is still open is the release, not the packaging:** the PPA upload needs a Launchpad
account and a signing key, which are the operator's to hold.

| Leg | State |
|---|---|
| Three packages build | **observed** — `faciolock`, `libpam-faciolock`, `faciolock-models`, plus two `-dbgsym` from `dh_strip` |
| `lintian` clean on the **binary** packages | **observed** — zero errors, zero warnings. The one warning it did raise, `no-manual-page`, is closed by `faciolockctl.1` |
| `lintian` clean on the **source** package | **not clean on the first run, and the distinction matters** — the binary `.changes` and the source `.changes` are different runs, and only the second one sees `vendor/`. Three findings, three different dispositions. *Fixed:* a DEP-5 `License:` line naming licences with no paragraph of their own — `Expat` is the canonical name for MIT, and `other-permissive` now has a stanza naming the four crates it covers. *Overridden:* `source-contains-patch-failure-file` on all 156 `Cargo.toml.orig` files, which is [B30](11-bug-log.md#b30)'s file seen from the other side. *Overridden and written down as an ITP blocker:* two **prebuilt Windows DLLs** in `libloading`'s test fixtures. Nothing here builds, links or runs them, and they are still prebuilt binaries without source. An override is not a fix and is not claimed as one. Re-run after the overrides: **clean** |
| A source package that could actually be uploaded | **observed** — `faciolock_0.1.0~ppa1.dsc` + a **47 MB** `.tar.xz`, `3.0 (native)`, declaring all three binaries. Well inside Launchpad's limits despite the 194 MB vendored tree |
| Multi-arch fields | **observed** — `faciolock` and `faciolock-models` `foreign`, `libpam-faciolock` `same`. `lintian` caught the combination that cannot hold (`Multi-Arch: same` depending on a package that declares nothing), and `fprintd` had already answered it on this machine: `libpam-fprintd` same → `fprintd` foreign. Read off `dpkg-query` rather than recalled |
| **`lintian` clean on both `.changes`, binary and source** | **observed** — zero output from either, after one fix, three justified overrides and the multi-arch correction |
| The whole test suite runs inside the package build | **observed** — `cargo test --release --workspace --locked` is `override_dh_auto_test`, and the five hardware tests stay `#[ignore]`d because a buildd has no camera |
| Contents match `install-local.sh` | **observed**, `dpkg -c` on all three, with the three deliberate divergences and nothing else |
| `pam-auth-update` profile at `Default: no`, `Priority: 512` | **verified** — the file has read that way since Phase 6 |
| `postinst`/`prerm` call `pam-auth-update --package` | **observed** in the built `.deb`, and copied verbatim from `libpam-fprintd`'s own scripts on this machine rather than reconstructed |
| The `faciolock` user is actually created on install | **observed after two wrong versions** — [B31](11-bug-log.md#b31). `dh_installsysusers` is not in this debhelper's sequence, so it needs the right filename *and* an explicit call. Independent confirmation: `Depends` gained `systemd \| systemd-standalone-sysusers \| systemd-sysusers`, which only `dh_installsysusers` adds |
| The socket is started, not merely enabled | **observed** in the generated postinst — `_dh_action=start` on install, `restart` on upgrade. A first draft passed `--no-start` for both units, which would have left the socket enabled and not listening until a reboot |
| Units under `/usr/lib/systemd/system`, not `/etc` | **observed** |
| No `/etc/faciolock/faciolockd.toml` shipped | **observed** — the directory ships, the file does not. Load-bearing: `probe::write_config` opens with `create_new(true)` (P5-e), so a shipped conffile would permanently break `diagnose --write` |
| The module links libpam | **observed** — `libpam-faciolock` depends on `libpam0g (>= 0.99.7.1)`, picked up from the real `DT_NEEDED` by `dh_shlibdeps` rather than declared by hand |
| Vendored build works offline | **observed** — `CARGO_NET_OFFLINE=true` and `--locked` throughout, against `vendor/` |
| Reproducible: two builds, identical `.deb` | **observed** — all five artefacts byte-identical across two independent runs, `-dbgsym` included, `sha256sum -c` on every one. `SOURCE_DATE_EPOCH` comes from the changelog, `--locked` pins the tree, and the vendored sources are the same bytes both times. **The caveat is stated rather than buried: both builds ran from the same path.** Nothing yet sets `--remap-path-prefix`, so a build from a different directory is untested and would be the next thing to fix |
| **Install the built packages, on the reference machine** | **observed 2026-08-31 22:57.** `install-local.sh --remove`, then `apt install` of the three `.deb`s. `apt` created `/etc/systemd/system/sockets.target.wants/faciolock.socket → /usr/lib/systemd/system/faciolock.socket` — the `dh_installsystemd` split doing its job — and the socket came back `srw-rw---- root faciolock-auth`. No `/etc/systemd/system/faciolock.*`, so the package's units are the ones in force rather than shadowed by the hand-install's |
| **The enrolment survives a hand-install → package swap** | **observed** — `faciolockctl list` reads `1000 csr 4676 bytes, 4 h ago` through the packaged binary against a store written by the hand-installed one. That is the upgrade-persistence path, for free |
| **The packaged daemon authenticates** | **observed** — `faciolockctl test csr` → **`Allow`**, 1,965 ms cold. Everything in that path is the package's: the daemon out of `/usr/libexec`, the models out of `/usr/share/faciolock/models` under `ProtectSystem=strict`, the socket from the packaged unit |
| **`diagnose` from the packaged binary** | **observed**, and as the *unprivileged* user, which is the one command that does not need root. Selected `/dev/video2`, rejected the RGB node and both metadata nodes, **alternation 1.00 at 61,420:1** — a far cleaner strobe than the 44.8:1 read in daylight, which is [B18](11-bug-log.md#b18) visible in the diagnostic rather than in a refusal |
| **Purge removes everything it installed** | **observed** — `apt remove --purge` took the units, both binaries, the module, the pam-configs profile and the models. `systemctl is-enabled faciolock.socket` → `not-found`, and the `sockets.target.wants` symlink is gone |
| **`prerm` un-enables the PAM profile** | **observed, and this is the leg that mattered most.** The profile was *enabled* in `common-auth` at removal time, and afterwards that file reads `auth [success=1 default=ignore] pam_unix.so nullok` — our two lines gone, and **`try_first_pass` gone with them**. [07](07-desktop-and-packaging.md#rolling-it-out-and-getting-back-out) flagged that `pam-auth-update` rewrites `pam_unix` when a Primary module sits above it; the removal reverted its own edit rather than leaving it |
| **What purge deliberately leaves** | **observed** — `/var/lib/faciolock` (the enrolment and its key), `/etc/faciolock/faciolockd.toml`, the `faciolock` user and the `faciolock-auth` group. dpkg says so out loud: `directory '/etc/faciolock' not empty so not removed`. All four are the designed answer to "a package that deletes a biometric template on a bad upgrade costs you a re-enrolment" |
| One thing purge did **not** take | **`/run/faciolock.sock`**, left as an orphaned inode with nothing listening. The `prerm` stops both units correctly; systemd's `RemoveOnStop=` simply defaults to `no`. It clears at reboot and systemd rebinds over it — the 22:57 install did exactly that over the hand-install's leftover — so nothing breaks. Fixed anyway with `RemoveOnStop=yes`, because "uninstall" is a leg of this phase's exit and a file the uninstall did not take is a file the uninstall did not take |
| **Unlock, on the packaged install** | **observed 2026-08-31 23:02:00**, and the journal distinguishes it from the `test` 37 seconds earlier better than any description could: `auth uid=1000 verdict=Allow reason=match similarity=0.904 agreeing=3 live=pass probes=3 **pid 100946 uid 1000** elapsed=1972`. The `test` at `23:01:23` carries `uid 0` — root asking on the CLI's behalf. This one carries **`uid 1000`**, an unprivileged session process asking about itself, which is `kscreenlocker_greet` and nothing else. Same signature as Phase 6's `19:20:33`, now through packaged units, a packaged daemon and a packaged module |
| PPA upload | **not attempted** — needs a Launchpad account and a signing key |

---

## Phase 8 — The settings panel (week 9)

**Started 2026-09-04.** Phase 7 shipped a system that works and can only be operated from a
root terminal. Everything a person does with Faciolock — enrol, test, remove — is behind
`sudo` or `pkexec`, so "install this" still means "open a terminal". The KCM has carried a
"(later)" beside it since [07](07-desktop-and-packaging.md) was written; this is it.

Scope was cut twice before any file was written, and both cuts are the point:

- **Not B18/B24.** Both need capture sessions — a floodlight-in-daylight recording, more
  prints, a second subject. They are the highest-value work in the project and none of it is
  desk work. The risk register still carries them and the README still names them.
- **Not the release.** The PPA upload needs a Launchpad account and a signing key.
- **GNOME/GDM: nothing installed, so nothing claimed.** Recorded as a
  [desktop compatibility table](07-desktop-and-packaging.md#desktop-compatibility) with one
  observed row and one untested one, and the two questions that decide it written out.

### The panel holds no privilege, because it did not need any (P8-a)

The obvious way to build a settings page for a root-only CLI is to give the page a way to be
root: a D-Bus service, a helper, a polkit action per operation. None of that was written,
because Phase 7 had already shipped the boundary. `org.faciolock.manage` annotates
`org.freedesktop.policykit.exec.path=/usr/bin/faciolockctl` and `faciolockctl`'s own error
message has told people to run `pkexec faciolockctl …` since Phase 5. The panel does exactly
that. It can do what you could do at a terminal and nothing more, and there is no new code
between a click and a capability.

The one thing it deliberately **cannot** do is switch face unlock on or off. That is
`pam-auth-update`, which is not our binary, so covering it means a second polkit action
authorising admin-level PAM reconfiguration — a wider boundary than a settings page has any
business opening. The panel reads `/etc/pam.d/common-auth`, reports the state, and prints the
command.

### `Op::Status` was already unprivileged, and nothing had ever called it that way (P8-b)

`serve::may_ask` has said `peer_uid == 0 || (subject == Some(peer_uid) && matches!(op,
Op::Auth | Op::Status))` since P6-d, and Phase 2 measured `Op::Status` at **0 ms, no camera
touched**. So a session process may ask whether *it* is enrolled, for free — and no caller
existed: `faciolockctl list` reads the 0700 store and is root-only.

`faciolockctl status` is that caller. It is what lets the panel show its state on open with no
password prompt, and it is one line of output with the state first so a GUI splits rather than
parses prose.

Writing it turned up a message that had been wrong since P6-d: the socket error still said
*"is not readable by this user — run as root"*, from when the socket was `0600 root:root`. It
is `0660 root:faciolock-auth` now, and the useful half of the answer is the group, not root.
`needs_root` carried the same fossil — "the socket is 0600" — when what actually needs root is
the 0700 store and `may_ask` refusing `Enroll` and `Clear` to everyone else.

### Enrolment blocks on stdin, and a GUI streaming stdout would hang forever (P8-c)

`prompt()` is `print!("{instruction}  [Enter] ")`, flush, `read_line`. Two properties, both
invisible until something other than a terminal reads it: it **waits for the operator between
every pose**, and the prompt **carries no newline**, so a reader waiting for a line waits
through the entire enrolment. A panel that only piped stdout to a text box would show an empty
box and a process that never finishes.

The fix is not to reimplement the loop in the panel. Off a terminal — `std::io::stdout()
.is_terminal()`, stdlib — `prompt()` emits one extra line first, `[pose 1/3] <instruction>`.
The panel keys on it, shows a Capture button, and clicking it writes the newline `read_line`
is blocked on. The terminal path is byte-identical to before. The marker is a contract, so it
has a test: reword it and the panel silently stops prompting, which is the failure that would
be hardest to find.

The alternative was parsing the trailing `[Enter] ` out of the prompt itself, which works
today and breaks the next time someone improves the wording.

### KF6 has no pure-QML KCM

Checked before designing around it: `kcmutils_add_qml_kcm()` takes a C++ `SOURCES` argument
and wants a `KQuickConfigModule` subclass. The KF5 KPackage-based QML-only module is gone. So
the KCM is C++ and CMake in a workspace that had neither — five files, of which one has real
content — and the base class was read off `kcm_users.so` on this machine (`KQuickConfigModule`,
not the `Managed` variant, which wants a `KCoreConfigSkeleton` this panel does not have).

"Both" — System Settings *and* a launcher entry — cost a `.desktop` file rather than a second
binary, because `kcmshell6` already opens any KCM as its own window. The generated System
Settings entry is `NoDisplay=true`; the shipped one is not. Confirmed by reading both files
out of the build tree rather than by expecting it.

**The first build failed on a missing header, and the header was not the problem.** Upstream's
own example links `KF6KCMUtilsQuick`, which is the target name *inside* the kcmutils source
tree and is not exported to consumers — the exported one is `KF6::KCMUtilsQuick`. CMake does
not treat an unknown name in `target_link_libraries` as an error; it assumes a system library
and passes `-lKF6KCMUtilsQuick` to the linker. So the include directories that target carries
never arrived, and the failure surfaced 40 lines later as
`KQuickConfigModule: No such file or directory`. Fixed by reading the exported target names
out of `KF6KCMUtilsTargets.cmake` on this machine, which is the same move that settled the
base class and the multi-arch fields in Phase 7: the answer is on the disk.

**The panel runs before it is installed.** `QT_PLUGIN_PATH=target/kcm/bin kcmshell6
kcm_faciolock` loads the plugin straight out of the build tree, which is how the first render
was checked with no root and nothing installed. It came up correctly on its unhappy path —
`/usr/bin/faciolockctl is not installed`, which was true, since Phase 7's purge took it — with
all three action buttons disabled and no QML warnings.

### The exit, and how the journal proves a click (2026-09-05)

**Exit: met at 20:00:55.** A face enrolled from a GUI, by someone who never opened a terminal.

Proving it needed a moment's thought, because the obvious evidence is not evidence. A panel
enrolment runs `pkexec faciolockctl enroll`, and `sudo faciolockctl enroll` runs the same
binary as the same uid — the daemon writes `uid 0` for both and cannot tell them apart. So
the journal alone says nothing about which one ran.

Two things together do say it:

```text
20:00:43  Started faciolock.service            <- socket activation, by the panel's request
20:00:45  enrol uid=1000 pose=1/3  pid 70441  elapsed=1954
20:00:50  enrol uid=1000 pose=2/3  pid 70441  elapsed=607
20:00:55  enrol uid=1000 pose=3/3  pid 70441  elapsed=608
20:00:55  enrol uid=1000 event=template-written poses=3 embeddings=9
```

**The poses are five seconds apart.** `elapsed` is the daemon's own work — 607 ms, 608 ms —
so the other four and a half seconds are a person reading a banner, moving, and pressing a
button. And the installed binary carries [B32](11-bug-log.md#b32)'s fix, which is what makes
that argument close: with stdin at EOF this enrolment would have **aborted** at pose 1 rather
than completing. It completed, so `read_line` returned three times, so `capture()` wrote three
newlines, so the button was pressed three times. Before B32 was fixed, a completed enrolment
would have proven nothing at all — an empty stdin produced the same four log lines.

The panel's own log pane is the other half, and it reads exactly as designed:

```text
Look straight at the camera.  [Enter]   captured  (1/3, 1954 ms)
Turn your head slightly to the left.  [Enter]   captured  (2/3, 607 ms)
Enrolled csr. Check it with:  sudo faciolockctl test csr
```

No `[pose 1/3]` lines appear, which is correct: `readTask()` consumes markers into the banner
instead of logging them. And the prompt sharing a line with the reply is the newline-less
`print!` being completed by the next `println!` — one whole line, logged once.

| Leg | State |
|---|---|
| `faciolockctl status`, unprivileged, live daemon | **observed** — `enrolled csr`, exit 0, no password prompt. The first caller of the `may_ask` path P6-d opened, four phases after it was opened |
| …and its refusal | **observed** — dropped from `faciolock-auth` and checked with `sudo -u csr`, because a re-run in the same shell keeps its supplementary groups until a fresh login and would have passed for the wrong reason. `refused this user — you are not root and not in the faciolock-auth group` |
| The panel reads true | **observed** — "Enrolled" against `status`, "Off" against `grep -c pam_faciolock /etc/pam.d/common-auth`, each checked independently rather than against each other |
| **Opening the panel does not light the emitter** | **observed.** The `status` call activated the daemon at 19:55:33 and it logged two lines, `models verified` and `listening`. No warm, no frames. [B29](11-bug-log.md#b29)/P6-e holds through a caller written three phases later, which is the only way that fix was ever going to be tested again |
| System Settings and the launcher | **observed** — `systemsettings --list` lists `kcm_faciolock`; the launcher entry installs and `desktop-file-validate` passes |
| **Enrol from the panel** | **observed 20:00:55**, above |
| **…and again from the packaged install** | **observed 20:20:21.** Four `.deb`s installed over a removed hand-install, then an enrolment through the panel: `elapsed=2043 / 654 / 772`, poses two and six seconds apart, and the panel's log pane carries the same three numbers. `dpkg -S` puts `/usr/bin/faciolockctl` in `faciolock` and both KCM files in `plasma-faciolock-kcm`; nothing shadows the units in `/etc/systemd/system`. The enrolment written at 20:00 by the hand-install was still readable at 20:20 through the packaged binary, which is the upgrade-persistence path again, for free |
| **B33's dependencies resolve** | **observed** — `apt` installed all four packages pulling *nothing* new. That is the fix working and the reason the bug was invisible: every `qml6-module-*` this panel imports was already on the machine, and would not be on a fresh one |
| **Purge takes the panel with it** | **observed** — all nine installed paths, the three new ones included: the plugin, both `.desktop` files and the lintian override. Units gone, `is-enabled` → `not-found`, System Settings and the launcher both de-register on the `desktop-file-utils` trigger. Kept on purpose and unchanged from Phase 7: `/etc/faciolock`, `/var/lib/faciolock`, the user, the group |
| **`/run/faciolock.sock` goes too** | **observed, and it is the first time.** Phase 7's purge found that inode left behind with nothing listening and fixed it with `RemoveOnStop=yes` — but that purge was the last one anyone ran, so the fix had been written and never watched. It works |
| The `prerm`'s `pam-auth-update --remove` | **not exercised this phase.** The profile was never enabled here — `grep -c pam_faciolock /etc/pam.d/common-auth` read 0 throughout — so that leg rests on Phase 7's observation and is not a fresh one. Said plainly because a purge that removes a profile nobody enabled has proved nothing about removing one somebody did |
| Four packages build | **observed** — `faciolock`, `libpam-faciolock`, `faciolock-models`, `plasma-faciolock-kcm`, plus three `-dbgsym` |
| `lintian` on **both** `.changes` | **clean, on the fourth attempt.** The first run was not, and what it found was not what it warned about — see below. Two justified overrides, and the override was confirmed *shipped* (`/usr/share/lintian/overrides/plasma-faciolock-kcm`, both lines) rather than inferred from the warnings going quiet, because [B31](11-bug-log.md#b31) is what a debhelper that does not run looks like |
| A source package that could be uploaded | **observed** — `faciolock_0.1.0~ppa2.dsc` + a 47 MB `.tar.xz` declaring all four binaries, with all five `kcm/` files inside it. Checked rather than assumed: the tarball is assembled from `git ls-files`, so an uncommitted source file disappears without a word and Launchpad builds something other than what was tested |

### The package worked here and would have been blank anywhere else (B33)

`lintian` came back with two warnings, both `desktop-command-not-in-package`, for `kcmshell6`
and `systemsettings`. Both are about `Exec=` lines and neither is the bug.

Chasing them meant asking how a KCM package is *supposed* to declare itself, and the answer
was already installed: `dpkg -s plasma-firewall` lists six `qml6-module-*` packages in its
`Depends`. Ours listed none. `ui/main.qml` imports five QML modules and the built package
depended on **not one of them**, because `kcmutils_add_qml_kcm` compiles `ui/` into a Qt
resource inside the plugin, where the imports are string data and `dh_shlibdeps` — which reads
`DT_NEEDED` out of ELF — cannot see them. The C++ half of a QML KCM declares its dependencies.
The QML half does not.

The package installs, registers, and launches to a **blank page**, with QML import errors on a
stderr nobody reads, because settings panels are not started from terminals. And **it cannot
fail on the machine that builds it** — those modules arrive with Plasma. Every observation in
the table above is true and none of it was evidence about anybody else's machine.
[B33](11-bug-log.md#b33).

That is the third time in two phases that the answer was on the disk rather than in a memory:
the multi-arch fields off `dpkg-query`, the CMake target name out of
`KF6KCMUtilsTargets.cmake`, and now the dependency list off a package that had the problem
first.

The two warnings that started it are a **false positive**, and lintian's own tag text says so:
*"does not match any of the executables shipped in the package."* It looks at this package and
not at `Depends`. A KCM ships no executable on purpose. Overridden — and unlike Phase 7's
three source overrides, this one is claimed as a fix, because the dependency exists and is
declared and lintian simply cannot see into another package to find it.

### The package build also found a flaky test, which the local runs could not

The build after the B33 fix **failed**, on a test rather than on packaging:

```text
---- tests::the_subject_defaults_to_the_invoking_user_not_root ----
flags are not names: "no username given, and $SUDO_USER is not set"
```

Two tests were mutating `$SUDO_USER` — the existing one sets it and asserts on it, and P8-b's
new one removes it — and `cargo test` runs them in parallel threads that share an environment.
Two owners of one global. It passed three times here before failing in the build, which is
what a race does rather than what a broken test does.

Fixed by merging them: one test owns the variable. Five consecutive local runs green
afterwards, because one green run proves nothing about a race.

Worth recording for a reason that outlives the test: `override_dh_auto_test` runs the whole
suite inside every package build, which is a fourth execution environment on top of the three
`cargo test` already sees, and it is the one that ran the tests in an order the others did not.

### B33, shown rather than argued — in a container, on this machine (2026-09-05)

The VM was owed for two jobs and they need different tools. The runtime-dependency half —
"does the panel work with only what we declared?" — turns out to be testable here, because Qt
ships `libqoffscreen.so`: `QT_QPA_PLATFORM=offscreen` renders with no display, so QML genuinely
loads and import failures genuinely surface. `ubuntu:26.04`, nothing installed, the four
`.deb`s:

| | |
|---|---|
| **A — as declared** | `apt install` exit 0, and it **pulled all five `qml6-module-*` packages in** on a machine that had none of them. `kcmshell6` exit 124 — still up when the timeout fired. Stderr: a locale notice, nothing else |
| **B — one declared module forcibly removed** | `dpkg --remove --force-depends qml6-module-org-kde-kirigami`, which leaves the KCM installed and intact: exactly the state B33 shipped in |

B is here because the description of B33 — "installs, registers, launches, blank page" — was
**reasoning, and reasoning is not what this project writes down as observed.** What it printed:

```text
plasma-faciolock-kcm: install ok installed      <- dpkg is perfectly happy
kcmshell6 exit: 124                             <- and the window stayed up
kf.kcmutils: Error loading QML file qrc:/kcm/kcm_faciolock/main.qml
kf.kcmutils:     main.qml:8:1: module "org.kde.kirigami" is not installed
```

Two things it settles that the argument had only guessed. The process **does not exit
non-zero** — it runs, shows nothing, and keeps running, so no watchdog, exit code or `dpkg`
status anywhere in the system registers a complaint. And the path is `qrc:/`, which is the
mechanism itself: the QML is a Qt resource compiled into the plugin, which is exactly why
`dh_shlibdeps` cannot see the imports.

The standing guard is `tools/check-qml-deps.sh`, first thing in `mk-source-package.sh`. It
resolves each `import` to its owning package via `dpkg -S` on the module's `qmldir` and fails
the build if one is missing from `Depends` — verified in both directions, because a check that
has only ever passed is not evidence. The container proves *this* package is right today; the
script is what stops the same bug arriving with the next `import`.

**What the container does not close.** It has no systemd, so socket activation, the units, and
`sysusers` creating the `faciolock` user on a first install are all still untested by it. The
VM is still owed — for less than before, and for the half that has not yet bitten.

**What B33 costs the phase, stated rather than buried:** the clean-VM install was traded away
at [Phase 7's exit](#the-models-are-gitignored-and-the-tarball-needs-them) for a
reference-machine install, on the argument that the removal half is the stronger half. B33 is
exactly the defect that trade cannot catch, and it is now owed twice.

---

## Beyond v1

- ~~KDE settings module (KCM)~~ — **Phase 8**
- Debian ITP → archive ([07 checklist](07-desktop-and-packaging.md#the-upstreaming-checklist))
- Independent security review — ideally someone who has reviewed PAM modules before
- GNOME/GDM support — [the two open questions](07-desktop-and-packaging.md#desktop-compatibility)
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
