# Faciolock — Knowledge Wiki

IR face authentication for Linux via PAM. Rust. Built to look and behave like a
platform component, not an add-on.

**Status: Phases 1, 3, 4 and 5 built; Phase 2 open on one criterion.**

**Phase 4 landed 2026-08-23** — the template store is no longer plaintext and no longer
keyed by a name. `<uid>.tpl` is XChaCha20-Poly1305 with the uid authenticated as associated
data, so a copied file does not decrypt, a file renamed to another uid does not decrypt, and
one flipped bit is an error rather than a slightly different face. The key is a **root-only
file, not the TPM** — [Q5 decided that](08-open-questions.md#q5--tpm-sealing-v1-or-later--decided-2026-08-23-later)
with the seam built, and [03 R6](03-pam-and-security.md#r6--templates-are-encrypted-the-key-is-a-file-not-the-tpm-v1)
now carries a table of exactly which attacks that stops and which it does not. Alongside it:
a persistent three-strike failure budget, a lockout released through `pam_sm_acct_mgmt`, and
one structured journal line per attempt.

**The failure budget is not a backstop any more, and that is the phase's real finding.**
R5 was written as the net under liveness. [B24](11-bug-log.md#b24) inverted it — a printed
face clears every enforced liveness check — so until something rejects a print, **the rate
limiter is the only control between a photograph and unlimited retries**. Nothing about the
design changed; what changed is what it is holding up.

Two smaller things the build settled. [04's data layout](04-architecture.md#data-layout) was
internally inconsistent — it said the templates were `root:root` while R7 runs the daemon as
`User=faciolock`, and the daemon has to read them; corrected, with the consequence (anything
running as `faciolock` can read the key) written down rather than left to be discovered. And
`PAM_IGNORE` was verified out of the shipped `libpam.so.0` rather than a header, because
`libpam0g-dev` is no longer installed here and `pam.rs`'s standard is that its constants are
read rather than recalled — which cross-checked the four already there.

**Phase 1.** `crates/faciolock-vision` takes frames in and produces a
verdict out — liveness, YuNet detection, alignment, SFace embedding, k-of-n matching — with
no camera, no sockets and no PAM. 78 tests, of which the ones that need neither the
recordings nor the models always run. Detection and alignment are pinned against
independent OpenCV references, which is what caught [B16](11-bug-log.md#b16).

**Phase 2 in progress.** `crates/faciolock-camera` (V4L2 capture, IR discovery) and
`crates/faciolock-ipc` (the wire types) are built. First contact with real hardware found
[B18](11-bug-log.md#b18) — every strobe statistic was a ratio, and ambient daylight
saturates a ratio — which is now fixed by moving to the difference domain and by an
**ambient gate**: above a measured floor this hardware reports "cannot verify" and PAM
falls through to the password, rather than guessing.

**The gate is at the wrong layer, and that is the open question of Phase 2.** It was
derived to protect `dark_ratio_max`, which genuinely dies in ambient light, but it sits in
front of the whole burst and so it also disables `falloff` — which is computed on the ON−OFF
*difference*, where the ambient pedestal cancels exactly, and which measurement now shows is
**unaffected by ambient right up to a pedestal of 51.7**. Daylight was written up as a
hardware ceiling; that was wrong, on a same-hardware fact (Windows Hello succeeds in the
room where Faciolock refuses, on this laptop). **[13](13-daylight.md)** holds that question
in one place, with the ways out ranked.

**The negative class exists now, and it broke the plan.** Rescoping the gate onto `falloff`
was the cheap way out, and it required knowing what `falloff` reads on a print — never
measured, "the one genuinely unproven claim in the project", open since day one. A magazine
cover settled it on 2026-08-19 for the price of eight minutes:

```text
flat_printed_face   falloff CV 0.2733    worst live face 0.2661    ->  the print reads HIGHER
```

The classes interleave, so **no threshold on `falloff` separates a live face from a printed
one**, and a flat printed face clears every enforced liveness check — dark-frame, response,
falloff, motion — and is detected by YuNet while doing it ([B24](11-bug-log.md#b24)). Nothing
unlocked: the magazine is not the enrolled user and matching still gates. The finding is
narrower and worse than an unlock, which is that liveness contributes nothing against a
print. [B21](11-bug-log.md#b21) predicted the direction — ink enters the ON−OFF difference
exactly like shape, so a photograph of a curved object carries the variance of a curved
object — and "closer to live" turned out to mean past it.

Phase 0's S5 therefore **exits on its second branch**, the one its criterion was phrased to
allow: evidence that falloff alone is insufficient.

`centre_edge_ratio` looked like the replacement — a directional question instead of a variance
one, prints at 0.876–0.918 against live at 1.446–1.774. It is **real but far thinner than it
measured**. The corpus test takes the worst of 41 pairs; the daemon judges 3. Per pair on the
detected box, the print's *best* reading is 1.443 against the weakest live capture's *worst* of
1.446 — a **~0.2% margin**, from one magazine and, on the other side, one subject: every live
capture in this project is the same person's head. No print exists at daylight ambient. And a
bare ceramic mug reads 1.800, above every live face, so whatever the statistic responds to,
"alive" is not it.

So **no threshold was taken, enforced or advisory**, and liveness still has no check that
rejects a print. [B24](11-bug-log.md#b24) carries the numbers — including four plausible
mechanisms for the split that were written down, checked, and retracted, which is most of why
that entry is long.

Auditing the statistic for promotion did find three defects that would have mattered the moment
it carried a threshold — `INFINITY` and a uniformly-negative field both passed a minimum, and a
short slice panicked — all now failing closed.

One thing [13](13-daylight.md) states that was not stated plainly anywhere before: the
ambient gate is a **UX hint, not a security control**, since [B20](11-bug-log.md#b20) showed
the attacker sets its input.

**`faciolockd` now answers, and Phase 2's exit criterion 4 is met** — `SOCK_SEQPACKET` at
0600, SHA256-verified models, a warm stream, and a real verdict from the real camera in
365 ms ([09](09-roadmap.md#phase-2--camera--daemon-skeleton-week-3)). The first run of it
found [B22](11-bug-log.md#b22): two crates each discarded the AGC settle frames, leaving 1
usable frame of 6, so **every** live authentication would have failed. 119 passing tests did
not see it, because the seam between two correct crates has no corpus.

The frame burst is now **measured at 404 ms** against a 130 ms estimate
([B19](11-bug-log.md#b19)), so the warm path is ~673 ms against a 400 ms target and
[06](06-performance-budget.md) is restated.

**Phase 3 is built, out of order and on purpose** — it is the only phase that needs no
camera, and Phase 2's remaining criterion is blocked on recordings that can only be made
after dark. `crates/faciolock-pam` is a cdylib whose dependency tree is exactly `libc` and
`faciolock-ipc`, asserted by a test; libpam's own functions are declared by hand rather
than taken as a crate, so the `.so` links against libpam without depending on it. The
daemon now checks `SO_PEERCRED` before it reads a request body and gives a non-root peer
**no reply at all**. The 200 KB module budget in [06](06-performance-budget.md) holds —
**164 KB resident**, against 353 KB on disk that is almost entirely `std`'s backtrace
symbolizer and never paged in.

**Every failure path is now observed through a real libpam** — 10 checks, 10 passed, each
`PAM_AUTHINFO_UNAVAIL` matched against the module's own journal line so it is the failure
it claims to be rather than a shared code. **The success path is not, and cannot be until
there is an enrolment**, which needs the camera; that is the whole of what is left in the
phase exit and it is not code.

The run also produced a number nobody was looking for, from a timing column added only so
the word "fast" would be checkable: the first request after a cold start waited **1779 ms**
to be accepted, because the daemon bound the socket and then warmed the camera on the same
thread. Every request paid it, including the ones that never touch a camera — a
`NoEnrolment` answered in 1753 ms cold against 0.4 ms warm. The warm now runs on its own
thread; the wait is **0 ms** and the re-run harness answers that same cold `NoEnrolment` in
**0.6 ms**. That was also quietly blocking Phase 6: the pre-warm agent
only works if a poke can be answered *while* the camera warms
([06](06-performance-budget.md#cold-start--measured-2026-08-09-found-by-the-phase-3-harness-fixed-the-same-day)). Two hang bugs were
caught in review first, both of them an unusable lock screen: the client set its socket
timeouts after `connect`, which does not return promptly when a listener's backlog is full,
and the daemon had no read timeout at all on an accepted connection. [B23](11-bug-log.md#b23)
also fell out of it — the socket's 0600 was being bought with a *process-global* `umask`.

**Phase 5 is built, and it is the one that unblocks the others.** Phases 3 and 4 both end
with the same open row, for the same reason: there has never been an enrolment on this
machine, so `sudo`-by-face and "a denied face moves the counter" have both been unreachable.
`faciolockctl` is that missing piece — `enroll` (three poses, one request each, accumulated
in the daemon), `list`, `remove`, `test`, `bench`, `diagnose`.

**[Q4 is closed, against its own recommendation](08-open-questions.md#q4--ipc-raw-unix-socket-d-bus-or-both--decided-2026-08-23-raw-socket-only)**
(P5-a): one IPC surface, the CLI speaks the same raw socket as PAM, polkit gates it at
`pkexec`. The hybrid had been recommended since day one and what it buys, weighed at the
moment of spending rather than in the abstract, is richer enrolment feedback — for `zbus` and
an async runtime inside the process that parses V4L2 buffers. The verdicts already on the
wire separate "it looked and refused" from "it could not judge", which is the distinction a
person acts on. `faciolockctl` came out at **779 KB stripped** against a ~2 MB budget, and
`cargo tree -p faciolock-pam` still reads exactly `libc` + `faciolock-ipc`.

`Op::Enroll` needed **no protocol change**: `Response` is four fixed bytes parsed inside a
root PAM process, and the obvious "poses remaining" field would have grown that parser for
the least security-sensitive feature in the project. The count is a shared constant instead,
and both ends count.

And `diagnose` produced a measurement worth keeping. In daylight it read **alternation 1.00
at a bright/dark ratio of 3.5:1**, against wiki/10's nocturnal 14,000:1 — the emitter is not
what changed, the floor is, and it is 5,000× higher. That is [B18](11-bug-log.md#b18) seen
through a second statistic, and it is why the tier classifier gates on the *pattern*:
**the alternation survives ambient infrared intact and the ratio does not**
([13](13-daylight.md#the-strobe-pattern-survives-daylight-the-ratio-does-not)). A classifier
keyed on the ratio would have called a perfectly strobing machine `ir-constant` because
somebody opened the blinds, which is the demotion P1-e exists to prevent.

**`sudo ./tools/enrol-try.sh` — 17 checks, 17 passed, first run, 2026-08-23.** A person
enrolled from three prompts, and **the project produced its first `Allow`**: a real face,
through the real daemon, over the real socket. Then three bursts of a face that was not the
enrolled one read `Deny, Deny, Deny` and took `ratelimit.db` to 3 — which closes **Phase 4's
last row**, open since 2026-08-09, and it needed Phase 5 to exist because it needed an
enrolment. Phase 3's `sudo`-by-face row is now unblocked rather than closed: what is left in
it is a decision about editing `/etc/pam.d/sudo`, not a missing capability.

The same run read the strobe after dark at **89,857:1**, against 3.5:1 from the same command
in daylight nine hours earlier — with **alternation 1.00 in both**. That pair, not either
number, is the evidence for gating the tier on the pattern.

**Still unobserved:** `bench`. It is also the one thing that wants a release daemon — an
enrolment pose cost 6.6 s in the debug build against wiki/06's 85 ms for SFace, so a debug
`bench` measures unoptimised `tract` and nothing else.

**Phase 6 is under way, and its first item unbuilt one of the plan's oldest assumptions.**
Socket activation and idle-exit are built and demonstrated — the daemon exits after 30 s
idle, `pidof faciolockd` is empty, and the next `connect()` brings it back — and the sandbox
reads **0.6 SAFE** against a target of 2.0. But **the pre-warm agent was specified against a
signal that does not exist** (P6-a). It pokes on the *lock* signal, and the cost it exists to
hide is paid on *return*, minutes later, after the daemon has already idle-exited. There is
no signal a session agent can subscribe to that fires when a person comes back but before the
greeter calls PAM. The ~800 ms stays visible, `idle_timeout_secs` is the only honest lever,
and it is config.

`cargo-fuzz` needed nightly this machine does not have, so the properties are also asserted
by seeded generators in `cargo test`. **Both of those tests failed their own first run and
both failures were the generator, not the parser** — random bytes reached the response
decoder 21 times in 200,000, and a random truncation landed on a frame boundary one byte in
64. Each test asserts a floor on how often it actually reached the code under test, and
without those two assertions both would have passed convincingly while exercising the early
returns.

**It is installed, and installing it found two bugs the tests could not.** `install-local.sh`
put the units, the `faciolock` user, the sandbox and the module on the reference machine on
2026-08-30, and the first command an operator runs on a fresh install —
`faciolockctl diagnose` — failed twice in a row. [B25](11-bug-log.md#b25): the courtesy
"daemon answering" line **started the daemon**, which took the camera two seconds before
`diagnose` tried to measure it. [B26](11-bug-log.md#b26): the installer had written a
placeholder config, and P5-e's never-overwrite rule made it permanent, so `diagnose --write`
could only ever refuse.

Both are the same shape and it is worth naming: **socket activation changed what `connect()`
means, and only the daemon's side of it was reviewed.** `serve::inherited` refuses a malformed
activation and the socket is not unlinked when systemd owns it — careful work, on the wrong
side of the socket. Nothing looked at the callers, where a read-only probe had silently become
a service start.

What the same install *proved* is the phase's other half, and a harness could not have: the
journal shows two full activations on one socket inode, the sandbox running the real workload
as `faciolock` — camera opened, both models verified under `ProtectSystem=strict`, warm in
335–339 ms, unchanged since Phase 2 — and both idle-exits landing at exactly 30 s.

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
| [12 — Falloff Calibration Log](12-falloff-calibration.md) | Recording sessions and readings for the falloff check; S5 is still open |
| [13 — Daylight](13-daylight.md) | The daylight question in one place: what blocks it, the ways out ranked by cost, what is already refuted |
| [14 — Tuning Constants](14-tuning-constants.md) | Every hardcoded number in the source, its value, and what it was chosen against |

## One-paragraph summary

A privilege-separated design copied from `fprintd`: a small, unprivileged,
socket-activated daemon owns the camera and the ML; a ~400-line PAM module owns
nothing but a Unix socket. Detection and embedding run on permissively-licensed
ONNX models (YuNet MIT / SFace Apache-2.0) so the whole thing is redistributable by
Debian — EdgeFace was ruled out when its *weights* turned out to be non-commercial
([B10](11-bug-log.md#b10)). Liveness is arithmetic, not a neural net: the IR
camera already alternates emitter-lit and dark frames, so differencing them rejects
screens, prints, and IR-floodlight attacks for the cost of a few `f32` reductions and
no writes to the device. Enrolled templates are encrypted with a root-only key file — the
TPM was deferred with the seam built (P4-a), and [03 R6](03-pam-and-security.md#r6--templates-are-encrypted-the-key-is-a-file-not-the-tpm-v1)
says plainly which attacks that stops and which it does not. Target: p95 unlock under 400 ms
wall-clock, zero RSS when idle.

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
| P3-a | **The socket path is a pam.d argument and never an environment variable.** The module runs as root in a process whose environment an unprivileged caller influences; a path taken from the environment would let that caller point it at a daemon they wrote, which answers `Allow`. A pam.d file is root-owned, which is the whole reason the knob is safe there | 2026-08-09 |
| P3-b | **Cancellation is checked at stage boundaries, not enforced mid-stage.** `POLLRDHUP` before the burst and before inference; an in-flight burst still finishes, so the camera can stay held ~400 ms past a cancel. Interrupting capture needs a worker thread and a flag, and nothing has measured that hold as a problem | 2026-08-09 |
| P3-c | **PAM-side deadline 3 s, applied to connect, write and read separately.** Above the daemon's own worst case (1500 ms capture + ~350 ms inference) so the daemon gives up first. It bounds `connect` too, which is not decorative — an `AF_UNIX` `connect` sleeps rather than fails when the listener's backlog is full | 2026-08-09 |
| P4-a | **The template key is a root-only file, not the TPM.** The store is encrypted either way; what was decided is where the key lives. `store::KeyProvider` puts the choice on disk *and* in the AEAD's associated data, so adding `Tpm = 1` later is a value old code refuses rather than a format break. The packaging objection that shaped Q5 turned out not to bind — `librust-tss-esapi-dev` is in the archive — but a C library in the process that parses attacker pixels still is a cost | 2026-08-23 |
| P4-b | **The lockout is released by `pam_sm_acct_mgmt`, not `pam_sm_setcred`.** Both are "the stack succeeded" signals; `setcred` is not called reliably by screen lockers, and the lock screen is exactly where a locked-out user is standing. The module returns `PAM_IGNORE` on every path, because an account-stack failure refuses a login outright instead of falling through to a password | 2026-08-23 |
| P4-c | **Backoff and lockout numbers are hardcoded, never config.** Same rule the vision thresholds already follow: a threshold reachable from an editable file is a threshold an attacker with one write sets to zero, and this one is read by the process that grants access | 2026-08-23 |
| P4-d | **`faciolock-store` is not built.** wiki/04 planned it; the store has one consumer. A crate boundary with one consumer costs maintenance and buys nothing. (The clause about the CLI reaching it over the socket was corrected by P5-b) | 2026-08-23 |
| P4-e | **Keyed by uid, translated once.** `users.rs` is the only place a username becomes a number; the store, the failure budget and the audit log never see a name. A name that does not resolve answers `NoEnrolment` — the same answer an unenrolled real user gets, so the socket cannot enumerate accounts | 2026-08-23 |
| P2-h | **No RGB camera in v1.** Measured: it cannot see the emitter (SNR 0.85 vs the IR camera's 1,500–15,000), so it cannot do strobe liveness. AND-ing it in would rely on a branch that reads *true* for attackers in daylight; OR-ing it in lets a photo unlock. Deferred to v2 as an explicit opt-in tier | 2026-08-06 |
| P5-a | **One IPC surface: the CLI speaks the same raw socket as PAM, as root, polkit-gated at `pkexec`.** Closes Q4 against its own recommendation. The hybrid bought richer enrolment feedback and cost `zbus` plus an async runtime inside the process that parses V4L2 buffers; the existing verdicts already separate "it looked and refused" from "it could not judge", which is the distinction a person acts on | 2026-08-23 |
| P5-b | **`faciolockctl` reads the template *directory* and never its contents.** A `list` answer is variable-length and `Response` is four fixed bytes parsed inside a root PAM process. A filename is a uid, a mtime is a date, `remove` is an unlink — and reaching any of it already requires root. Corrects the second clause of P4-d; the store still has one consumer and one implementation | 2026-08-23 |
| P5-c | **`test` and `bench` spend the real failure budget and give it back.** They go through `Op::Auth` exactly as PAM does, then send `Op::Clear` — something the CLI can already do as root. The alternative was a no-count path inside the daemon, and after [B24](11-bug-log.md#b24) the budget is the defence rather than a backstop behind liveness, so a bypass in it is not a thing to add for a benchmark | 2026-08-23 |
| P5-d | **Enrolment is three poses, accumulated in the daemon, with a 60 s expiry.** One `Op::Enroll` per pose and no protocol change: `ENROL_POSES` is a shared constant because both ends count. The daemon accumulates rather than the CLI because a CLI that gathered embeddings and handed them back would make the store write a function of the peer's bytes | 2026-08-23 |
| P5-e | **`diagnose` only ever asserts a tier upward.** It writes `ir-strobe` on a clean strobe and writes nothing otherwise. "No strobe here" and "somebody is pointing a floodlight at this" are the same measurement, and [B18](11-bug-log.md#b18) showed daylight alone produces it — so an automatic demotion would hand back the attack P1-e exists to prevent | 2026-08-23 |

| P6-a | **The pre-warm agent is deferred, and not for cost — the design does not work.** It cannot reach a 0600 root socket from a user session, and worse, it pokes at the wrong end of the absence: it warms on *lock*, and the ~800 ms it exists to hide is paid on *return*, after the daemon has idle-exited. No session signal fires when a person comes back but before the greeter calls PAM. `idle_timeout_secs` is the lever instead, and it is config because the trade — ~800 ms against a resident process holding a camera — is an operator's to make | 2026-08-29 |
| P6-b | **PAM ships as one `pam-auth-update` profile, not six `pam.d` files.** All six services include `common-auth` and `common-account` (verified, not assumed), so one profile reaches all six and one `--remove` takes it back — the supported uninstall a hand-edited file does not have. The profile gained an `Account:` section wiki/07's sketch lacked: without it `pam_sm_acct_mgmt` is never called, so P4-b's lockout release never runs. **A first trial still goes in `/etc/pam.d/sudo` by hand**, because the profile is all-or-nothing and a trial you can only evaluate by locking your own screen is not a trial | 2026-08-29 |
| P6-d | **R2 is now "root asks about anybody, anybody else asks about themselves" — enforced on `SO_PEERCRED`, not on the socket mode.** The socket becomes `0660 root:faciolock-auth` because [Q10](08-open-questions.md#q10--the-kde-lock-screen-cannot-reach-the-socket-open-it-or-drop-the-leg--decided-2026-08-31-open-it-option-a) measured that `kscreenlocker_greet` is not setuid and runs in the user's session, so root-only means the lock screen can never show a face. `Auth` and `Status` only: `Clear` from a session process is an unlimited supply of face attempts and `Enroll` is a template write. The cost, stated rather than buried — anything running as you can turn the emitter on and spend your budget, which is where `fprintd` already sits. The gain is that a wrong file mode now widens *who may ask about themselves*, not *what they may ask* | 2026-08-31 |
| P6-c | **Idle-exit is honoured only when the socket was inherited from systemd.** A self-bound daemon that exits on a timer has nothing to reactivate it — that is not an idle-exit, it is a crash on a schedule, and it would take every `tools/*-try.sh` harness with it. Same file, same timeout, different meaning depending on who owns the socket | 2026-08-29 |

Still open: Q9 (name), **whether a lockout should decay or clear on a successful face**
([Q10's knock-on](08-open-questions.md#the-knock-on-and-it-is-not-fixed-the-lock-screen-cannot-clear-a-spent-budget)
— the lock screen cannot clear a spent budget, so face unlock stays off until something
authenticates through a root stack), and whether the real recordings are ever published (deferred to ITP —
publishing is irreversible, so it is not decided early). Q5 closed 2026-08-23 (P4-a) and
names what would re-open it; Q4 closed 2026-08-23 (P5-a) and names what would re-open it.

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
| A template copied to another machine is unusable | **verified 2026-08-23**, in CI. There is no second machine here, so "another machine" is modelled as what one *is* to this format — a different key — and the test asserts the two stores genuinely generated different keys, so the refusal is not passing for an unrelated reason. A template renamed to another uid fails too, which is the implantation half |
| The failure budget survives a daemon restart | **verified 2026-08-23** — `sudo tools/ratelimit-try.sh`, 23/23, against the real socket: the file outlives `kill -9` and a restarted daemon refuses on it. That the write lands *before* the reply is shown indirectly, by `Op::Clear` removing the entry from disk while the daemon still runs; the direct version needs a failure the daemon recorded itself, which needs a camera |
| A locked-out user is refused **before** the camera and the store | **verified 2026-08-23** — same user, same daemon, answers `NoEnrolment` the instant the lockout is cleared, so the budget is what answered first |
| A denied face moves the failure counter | **verified 2026-08-23**, by Phase 5's harness rather than Phase 4's — it needed an enrolment, which needed `faciolockctl`. Three bursts of a face that was not the enrolled one: `Deny, Deny, Deny`, `ratelimit.db` reading `1000 3 …`, all three logged `counted=1`. P2-g's exemption held at the same time: nothing that never saw a face was counted |
| The daemon leaves no process behind when idle | **verified 2026-08-29** — under `systemd-socket-activate` against the shipped binary: `socket-activated`, camera warm in 339 ms, `idle for 3 s, exiting`, `pgrep -x faciolockd` empty, and the socket inode still present. Measured with a 3 s timeout rather than the shipped 30 so it can be watched; the mechanism is under test, not the number. **Reactivation on that surviving inode is a separate claim and is not yet verified** — the harness `exec()`s the daemon rather than supervising it, so only an installed `faciolock.socket` can show it |
| The daemon's sandbox meets its target | **verified 2026-08-29** — `systemd-analyze security --offline=true` reads **0.6 SAFE** against a target of 2.0 ([03 R7](03-pam-and-security.md#as-shipped--measured-2026-08-29)). Offline against the unit file, so it is reproducible without installing anything |
| The ambient gate keys on a quantity **only the attacker can raise** | **refuted 2026-08-09** — presenting a bright object 40 cm from the lens collapses the floor 2,170×: the operator's own face read 28.221 and was *gated*, a ceramic mug in the same minute read 0.013 and *passed*. The AGC shortens exposure for a bright near object and the pedestal follows, so the attacker sets the gate's input ([B20](11-bug-log.md#b20)) |
| Falloff separates a live face from a **flat print** | **still unmeasured, after three recording attempts.** A flat blank page was captured 2026-08-09 and cannot settle it: with no face to detect, the ROI must be chosen, and across valid placements CV spans 0.036–1.71 (median 0.249) against a worst live of 0.2661 — wider than the live band, so the choice decides the answer ([B21](11-bug-log.md#b21)). Needs a flat presentation of a FACE, so detection supplies the ROI. A blank page is an *optimistic* stand-in, not a conservative one: print albedo adds variance falloff cannot distinguish from depth |
| The camera reports **exposure time** per frame, which would fix B20's gate | **refuted 2026-08-09.** The `UVCM` metadata node streams, and over 30 frames it carries **zero `CaptureStats`**. This firmware does not report its integration time, so pedestal ÷ exposure — the one candidate for a gate input the attacker cannot pull down — has no input here ([10/Probe 8](10-hardware-probe.md#probe-8--the-metadata-node)) |
| The camera reports **per-frame illumination**, ON or OFF | **verified 2026-08-09**, and unprompted: the tag is present in `DISABLED`, no mode change needed. Validated against the strobe 26/26, mean(ON) 99.90 against mean(OFF) 74.33. Hardware ground truth for the phase `frame::classify` infers from luma, which removes the 3.5-levels-of-headroom fragility in [B18](11-bug-log.md#b18). Robustness, not daylight; unbuilt, and needs its own decision entry |
| Hardware self-strobes, every frame | **measured**, 4 independent runs, and now in daylight too — the emitter never stops |
| ~~15 ON/OFF pairs/sec~~ | **withdrawn.** 30 fps was the *advertised* rate; delivered is 12–15 fps, so ~6–7 pairs/sec ([B19](11-bug-log.md#b19)) |
| Dark-frame integrity rejects an IR floodlight | **verified at negligible ambient IR only** — 4 orders of margin and 0 false rejects across 210 live pairs, all nocturnal. In daylight the same check rejects **100% of honest captures** (0.664 against a 0.001 threshold). The check is a ratio and ambient light is an offset ([B18](11-bug-log.md#b18)) |
| The strobe is detectable in daylight | **verified** — as a *difference*: SNR 549 whole-frame, 882 over the face box, sign regularity 1.000. `classify` was rewritten to measure it ([B18](11-bug-log.md#b18)) |
| **Magnitude** statistics can distinguish a floodlight from a live face above ambient ~1 | **refuted, by a matched pair** — same room, same position, 90 s apart, ambient within 1% (13.91 vs 14.05). Every magnitude tried — ratio, SNR, response — ranked the attack at or above the live face. `dark_ratio_max` cannot be rescued ([B18](11-bug-log.md#b18)) |
| ~~That refutation makes daylight a **hardware** ceiling~~ | **withdrawn 2026-08-08.** Three errors: the 940 nm-optics explanation is refuted by its own premise (Windows Hello succeeds in that room *on this same laptop*); both captures in the matched pair contain the operator's live face, so it compared live against live and no spoof was present; and every candidate in it was a magnitude, which is the class that provably fails |
| **Falloff is ambient-invariant** — the pedestal cancels in the ON−OFF difference | **verified across the corpus.** Daylit live band CV 0.307–0.385 sits *inside* the nocturnal band 0.266–0.348; the problem room reads 0.3071 mid-band and a capture at pedestal 51.7 reads 0.3847, the corpus maximum. The gate refuses bursts this check can still read ([B18](11-bug-log.md#b18)) |
| Falloff can cover for `dark_ratio_max` against IR injection | **refuted, same table** — attack captures read live-like (0.3173, 0.3963) because they contain live faces. Falloff measures subject *geometry*; it cannot see injected IR |
| The camera's **face-authentication mode** would lower the ambient floor | **refuted by measurement.** The IR camera advertises `MSXU_CONTROL_FACE_AUTHENTICATION` and Linux had never set it — every corpus capture was taken in `DISABLED`. Setting `ALT_FRAME_ILLUM` works, persists, and moves the floor not at all: 20.41 baseline vs 18.96/20.69/21.21/21.09 ([10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode)). **Confirmed again 2026-08-09** in the bright room at ~2× the ambient and in reverse order, so drift worked against the hypothesis: 39.206 `DISABLED` vs 33.404/38.872 `ALT`, the mode's own repeat spread exceeding the between-mode gap, amplitude flat at 59.6–60.5. Coherent, because this sensor already self-strobes 24/7 — `DISABLED` *is* alternating illumination and the flag is bookkeeping |
| Faciolock works in a normally lit daytime room | **not yet, and it stays a non-goal pending calibration** ([01](01-goals-and-scope.md)). The blocker is the missing negative class, not the sensor: `falloff` enforces a 0.05 flatness floor against a live band bottoming at 0.2661 because **no print has ever been recorded**. Opening the gate today trades a measured refusal for an unmeasured acceptance |
| The RGB camera could supply liveness where IR cannot | **refuted by measurement** — it has an IR-cut filter and cannot see the emitter at all: strobe amplitude 0.049 and SNR 0.85 against the IR camera's 48–81 and 1,500–15,000 |
| YuNet detects a face in **daylight** IR | **verified** — and the face-box emitter response is 2.2× the whole-frame one, as inverse-square predicts |
| One contaminated pair sinks a burst (conjunctive aggregation) | **verified** — replayed in CI on synthetic and recorded captures |
| YuNet detects a face in **near-infrared** | **verified** — 4/4 live captures, 0 on an empty room, 0 on the floodlight attack |
| SFace embeddings discriminate on **near-infrared** | **verified** — genuine cross-capture cosine 0.60–0.90 against a 0.363 threshold |
| Falloff is measurable over a real face box | **verified** — 0.266–0.313, [B7 closed](11-bug-log.md#b7) |
| ~~Response test rejects screens~~ | **refuted** — a phone reads D=36 against a live minimum of 48.6, not ~0. The statistic is whole-frame, so it measures the room ([B17](11-bug-log.md#b17)) |
| A phone screen showing a photo is rejected | **verified** — but by *detection* (`NoFace`), not by any liveness check |
| **Falloff test rejects printed photos** | **untested, and deliberately deferred (2026-08-04).** [B7](11-bug-log.md#b7) is closed; no print has been recorded. The check therefore enforces only a degenerate-flatness floor and claims nothing about prints |
| False accept rate | **unmeasurable** — the corpus has one subject. The 0.363 cosine threshold is paper-derived || Falloff separates a live face from a **flat print** | **undetermined.** Three recording sessions; no reading that settles it. A flat page was captured on 2026-08-09, but with no face in it YuNet supplies no ROI, and across the 501 placements lying fully on the page the CV spans 0.0361–1.7116 (median 0.2486) against a live band of 0.2661–0.3847 — so the answer depends on the placement rule. Resolving it needs a printed face, so that detection supplies the ROI as it would in the pipeline ([12](12-falloff-calibration.md)) |
