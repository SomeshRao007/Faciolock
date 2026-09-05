# 11 — Bug Log

Bugs hit and fixed, newest first. Kept because several of these are **design** bugs
found by measurement, not typos — the reason they were caught at all is that Phase 0
exists, and the reason they are written down is so the same class gets caught again.

Severity is about what the bug would have cost if it had shipped, not how hard it was
to fix.

| # | Bug | Severity | Status |
|---|---|---|---|
| [B32](#b32) | **A closed stdin enrols three identical poses.** `read_line` answers `Ok(0)` at EOF and `prompt()` discarded the answer, so `enroll < /dev/null` fired three camera bursts back to back with nobody moving between them, and saved the result as a three-pose enrolment | **high** | fixed; `prompt()` returns false on EOF and `enroll` abandons. Predates the GUI by three phases and was found by asking what a Stop button would do |
| [B31](#b31) | **The package shipped the sysusers file and never ran it — twice, for two different reasons.** `dh_installsysusers` reads only `debian/package.sysusers`, *and* it is not in this debhelper's `dh` sequence at all. So the `faciolock` user is never created and the daemon cannot start | **high** | fixed; `debian/faciolock.sysusers` **and** an explicit `execute_after_dh_install: dh_installsysusers`. The build was green through both wrong versions |
| [B30](#b30) | **`dh_clean` deleted the vendored tree's integrity data.** Every `cargo vendor` crate carries a `Cargo.toml.orig` that `.cargo-checksum.json` hashes, and `dh_clean` sweeps `*.orig` as patch leftovers | medium | fixed; `dh_clean -X.orig`. Found by the first package build, which is what package builds are for |
| [B29](#b29) | **Every cron job lit the IR emitter for 30 s.** The account hook activates the daemon and activation warmed the camera, so a `run-parts` at :17 turned the lens on | **high** | fixed; the startup warm is deleted, the camera opens only for a request that needs it |
| [B28](#b28) | **`bench`'s first sample is the daemon cold start**, and at n=20 that sample *is* the p99 and the max | **high** | fixed; one unmeasured warm-up first, as [B15](#b15) |
| [B27](#b27) | **One refused enrolment pose disabled idle-exit permanently** — the daemon fell into a blocking `accept` and never re-polled | **high** | fixed; the decision is a function now, and asserted |
| [B26](#b26) | The installer created a **placeholder config**, which permanently blocked `diagnose --write` | medium | fixed; the installer creates the directory and nothing else |
| [B25](#b25) | **`diagnose` started the daemon that then took the camera it was about to measure** | **high** | fixed; it stats the socket instead of connecting to it |
| [B24](#b24) | **A flat printed face passes every liveness check.** `falloff_cv` cannot separate it — the print reads *above* the worst live face | **critical** | design finding, **open**. S5 answered on its bad-news branch. `centre_edge_ratio` does separate, but by ~0.2% at the burst length the daemon actually uses, from n=1 print and n=1 subject — no threshold taken. Liveness has **no** check that rejects a print |
| [B23](#b23) | The socket's 0600 was bought with a **process-global** `umask`, and an unrelated directory inherited it | medium | fixed by bind-then-rename; `umask` removed entirely |
| [B22](#b22) | The AGC settle frames were discarded **twice**, leaving 1 frame of 6 | **high** | fixed at the daemon's config seam; found by the first end-to-end run |
| [B19](#b19) | Corpus sidecars recorded the **requested** frame rate as if it were measured | medium | fixed; delivered rate is 12–15 fps, never 30 |
| [B18](#b18) | Every strobe statistic is a **ratio**, and ambient IR saturates a ratio | **critical** | `classify` fixed; ambient **gate** shipped; threshold provisional |
| [B17](#b17) | Response test does **not** reject a screen — the statistic is whole-frame | **high** | design finding, open |
| [B16](#b16) | Alignment sampled at a **half-pixel offset** from OpenCV's convention | **high** | fixed; found only by an external golden |
| [B15](#b15) | Latency benchmark ran inside the CPU's boost window and reported **half** the real cost | medium | corrected; warm-up + 50 samples |
| [B14](#b14) | Detection threshold copied from OpenCV rejected an **angled live face** | medium | fixed, set from the corpus |
| [B13](#b13) | Detector input size chosen from a latency table, never from **accuracy** | **high** | fixed, 192 → 320 |
| [B12](#b12) | Detected on an emitter-**OFF** frame — found zero faces in every capture | medium | fixed, phase measured not assumed |
| [B11](#b11) | Benchmarked `tract` against a published figure from **another machine** | medium | corrected by measuring both on one box |
| [B10](#b10) | EdgeFace **weights** are CC BY-NC-SA, not BSD-3 — DFSG-incompatible | **critical** | Q1 reversed to SFace |
| [B9](#b9) | Dark-frame check defeated by a *pulsed* IR source at per-pair granularity | **critical** | fixed by design change |
| [B8](#b8) | Liveness cannot run before detection, but the pipeline says it does | **high** | wiki corrected |
| [B7](#b7) | Centre-crop ROI does not cover the face — falloff test not evaluable | **high** | **CLOSED** — real face box, Phase 1 |
| [B6](#b6) | `by-id` symlink resolves to the *wrong camera* | **high** | fixed |
| [B5](#b5) | ffmpeg silently duplicates frames when writing to disk | **high** | fixed |
| [B4](#b4) | Capture file with no sidecar joins the corpus undocumented | medium | warns |
| [B3](#b3) | `2>/dev/null` hid a missing binary as "no evidence" | medium | documented |
| [B2](#b2) | Half the `/dev/videoN` nodes are not cameras | low | fixed |
| [B1](#b1) | Dead conditional in `build_pairs` | trivial | removed |

---

## B32 — A closed stdin enrols three identical poses {#b32}

**Severity: high.** The enrolment loop pauses between poses so you can turn your head:

```rust
print!("{instruction}  [Enter] ");
let _ = std::io::stdout().flush();
let mut line = String::new();
let _ = std::io::stdin().read_line(&mut line);   // <- the bug is the `let _`
```

`read_line` returns `Ok(0)` at end of file. It is not an error, so it does not surface as
one; the return value was discarded, `prompt()` returned, and the loop went straight to
`Op::Enroll`. With stdin closed there is nothing to block on **ever again**, so all three
poses fire back to back — three real camera bursts, roughly a second apart, of a person who
was never told to move — and the daemon writes them as a complete enrolment.

The resulting template is three near-identical samples of one pose. Every one of them is
genuinely yours, so nothing rejects it and nothing logs a complaint. What it costs is the
thing the three poses were for: an enrolment that only recognises you sitting exactly as you
were at that second, which reads later as a bad false-reject rate rather than as a bad
enrolment.

`faciolockctl enroll < /dev/null` has done this since Phase 5. Nothing found it because
nothing had reason to run enrolment without a terminal, and it never fails — it succeeds at
the wrong thing.

**Found by asking what a button would do.** The KCM's Stop button cannot signal its own
child: after `pkexec` authorises, `faciolockctl` runs as root and an unprivileged panel
gets `EPERM` from `terminate()`. The one stop an unprivileged parent *does* have over a root
child is closing its stdin — which is what sent us to look at what a closed stdin does, and
the answer was three camera bursts.

**Fix:** `prompt()` returns `false` on `Ok(0)` and `enroll` abandons with
`stdin closed after N of 3 poses`. That makes the panel's Stop button a clean abort rather
than an accidental speed-run, and it makes the CLI honest for anyone piping into it.

The class is the one [B3](#b3) is also in: a discarded result that is not an error. `let _ =`
on an I/O call is fine for a `write` whose failure changes nothing, and is a bug on a `read`
whose count is the only signal that there is nothing left to read.

---

## B31 — The package shipped the sysusers file and never ran it {#b31}

**Severity: high**, and the interesting part is that **the build was green**. `dh_clean`
([B30](#b30)) failed loudly and was fixed in a minute. This one produced three well-formed
`.deb`s, a clean `lintian` run, and a package that cannot start the daemon it installs.

`data/sysusers.d/faciolock.conf` was listed in `debian/faciolock.install`, which is the
obvious thing to do with a file that belongs in `/usr/lib/sysusers.d/`. It works, in the
sense that the file lands in the right place with the right mode. What it does not do is
tell anything to *read* it:

```text
$ dpkg -c faciolock_0.1.0~ppa1_amd64.deb | grep sysusers
-rw-r--r-- root/root  887  ./usr/lib/sysusers.d/faciolock.conf     <- shipped

$ grep sysusers debian/faciolock/DEBIAN/postinst
                                                                    <- nothing
```

`dh_installsysusers` reads exactly one input, `debian/package.sysusers`, and from it does
two things: installs the file *and* writes a `systemd-sysusers` call into the postinst.
Route the file in any other way and you get the first half silently.

**Renaming the file to `debian/faciolock.sysusers` produced an identically broken package**,
which is the part worth writing down. Calling `dh_installsysusers` by hand in the build tree
worked — file installed, snippet generated — so the helper was doing its job and something
was not calling it. It is simply **not in the sequence**:

```text
$ dh binary --no-act | grep -i "sysusers\|tmpfiles\|systemduser"
   dh_installtmpfiles
   dh_installsystemduser
```

`dh_installtmpfiles` is there. `dh_installsystemduser` is there. `dh_installsysusers` is
not, on debhelper 13.31ubuntu1 with `debhelper-compat (= 13)`. So the fix is two changes,
not one: the right filename **and** an explicit `execute_after_dh_install:
dh_installsysusers`.

There is no dpkg trigger to fall back on either. `systemd`'s trigger list on this machine
covers `/usr/lib/systemd/catalog`, `binfmt.d` and `sysctl.d`, and **not** `sysusers.d`:

```text
$ cat /var/lib/dpkg/info/systemd.triggers
interest-noawait /usr/lib/systemd/catalog
interest-noawait /usr/lib/binfmt.d
interest-noawait /usr/lib/sysctl.d
interest-noawait libc-upgrade
```

So `systemd-sysusers.service` at next boot is the only thing that would ever have created
the user, and "reboot before the package works" is not an install.

**Fixed:** `debian/faciolock.sysusers` (a symlink to `data/sysusers.d/faciolock.conf`, so
the users stay declared once), the `.install` line removed, and the explicit call in
`debian/rules`. The postinst now carries `systemd-sysusers faciolock.conf`.

The class is worth naming, and it is not "wrong filename". **The absence of a build step is
invisible.** A step that runs and fails is a red build; a step that never runs produces a
package that passes `lintian`, installs without error, and does nothing. Both wrong versions
here built three well-formed `.deb`s. The only thing that distinguishes them from the right
one is a grep for a snippet that should be in a file — which is to say, you have to already
suspect it.

That is also why the check belongs before the fresh-VM run rather than in it: the missing
half only surfaces on a machine that has never had the package, which is the most expensive
run to reach and the worst place to learn that a maintainer script was never generated.

---

## B30 — `dh_clean` deleted the vendored tree's integrity data {#b30}

**Severity: medium.** It cannot ship — the build fails rather than producing a wrong
package — but it fails in a way that reads as a corrupt vendor directory rather than as a
tool doing its job, and the first instinct is to re-run `cargo vendor`, which produces the
same result.

The first package build of the project, 2026-08-31:

```text
cargo build --release --workspace --locked
error: failed to calculate checksum of: …/vendor/v4l-0.14.0/Cargo.toml.orig
Caused by:
  No such file or directory (os error 2)
```

`cargo vendor` writes a `Cargo.toml.orig` into every crate it vendors — the manifest as
upstream published it, before cargo's own rewriting — and lists it in that crate's
`.cargo-checksum.json`. `dh_clean` sweeps `*.orig`, `*.rej` and `*~` as leftovers from a
patch that did not apply. Both are correct in isolation. Together, `debian/rules clean` runs
before `debian/rules build`, so every vendored crate loses a file its own checksum manifest
insists exists, and the build dies on the first one alphabetically.

**Fixed:** `override_dh_clean: dh_clean -X.orig`.

The general shape is worth keeping: a build that vendors third-party source into the source
package inherits every assumption the packaging tools make about what a file extension
means. `.orig` means "patch leftover" to debhelper and "upstream manifest" to cargo, and
nothing warns that both are true in the same directory.

**This is what a package build is for.** Nothing in `cargo test`, `install-local.sh` or
`activation-try.sh` could have found it, because none of them go anywhere near `dh_clean`.
The phase item that reads "four binary packages" was worth building rather than designing.

---

## B29 — Every cron job lit the IR emitter for thirty seconds {#b29}

**Severity: high.** Not for what it broke — nothing broke — but for what it is: a camera that
turns itself on several times an hour on an idle laptop, with no authentication anywhere near
it. Reported by the operator as "the cam starts every now and then and fires up IR rays", one
day after the `pam-auth-update` profile was enabled.

### What happened

Four steps, and each one is defensible alone.

1. `pam-auth-update` writes **both** halves of the profile. `Account-Type: Additional` puts
   `account optional pam_faciolock.so` into `/etc/pam.d/common-account`, which is where
   [P4-b](04-architecture.md)'s lockout release has to live.
2. `/etc/pam.d/cron` `@include`s `common-account`. Debian's cron calls `pam_acct_mgmt` for
   every job it runs, so `pam_sm_acct_mgmt` → `clear_lockout` fires on each one and
   `connect()`s to the socket to send `Op::Clear`.
3. **Under socket activation a `connect()` is a service start** — the same fact that produced
   [B25](#b25), arriving from a different direction.
4. The daemon then warmed the camera in a thread *before looking at the request*, and
   `Session` holds the `Capture` for the life of the process. So the stream stayed on for the
   **entire 30 s idle window**, not for the 340 ms of the warm burst.

From one hour of the reference machine's journal, with nothing but stock Debian cron entries:

```text
17:09:01  Started faciolock.service          <- phpsessionclean
17:09:02  pid 12418 uid 0 left before the answer (Broken pipe (os error 32))
17:09:03  camera warm on .../video2, 6 frames in 336 ms
17:09:32  idle for 30 s, exiting
17:17:01  Started faciolock.service          <- run-parts /etc/cron.hourly
17:30:01  Started faciolock.service          <- the anacron kick
```

Three activations in twenty-one minutes, three IR bursts, ~90 s of emitter, ~300 MB-seconds of
model load, and **not one of them wanted a face**. The `Broken pipe` line is the module's own
300 ms `CLEAR_DEADLINE` expiring while the daemon it just started is still SHA256ing 39 MB of
model — cron gave up before the answer it triggered all of this to receive.

`common-auth` is innocent here: cron never calls `pam_authenticate`. The auth hook did not run.

### The fix

**Delete the warm thread.** Thirteen lines, and there was nobody left to lose by it:

- [P6-a](09-roadmap.md#the-pre-warm-agent-was-specified-against-a-signal-that-does-not-exist-p6-a)
  had already established there is no signal that means "a person is about to return", so the
  warm was firing on "somebody connected", which is not the same event and on this machine was
  usually cron.
- PAM runs the **auth** stack before the **account** stack, so a `Clear`-triggered warm can
  never precede the `Auth` in the same login — the only order in which it would have helped.
- On a cold `Auth` it is a *cost*, not a saving: the arriving request waits on the session
  mutex for the warm's full burst and then takes its own. The 16:54 sample won the race and
  paid `elapsed=1782`; had it lost, it would have paid that plus a burst.

What remains is the honest cost, and P6-a always said it stays visible: the first
authentication after an idle-exit pays the model load plus the device open plus its burst.

Measured after the deletion, without root and without disturbing the installed daemon: the
patched binary self-bound on a scratch socket, left listening for two seconds, holds

```text
/proc/PID/fd:  socket:[84481]   /dev/null   the log   (and nothing else)
```

**Confirmed on the installed daemon 2026-08-31**, and by the same cron job that reported it.
`/usr/libexec/faciolock/faciolockd` replaced at 18:39; since then **six activations, zero
`camera warm` lines** — including `19:09:01`, the `phpsessionclean` tick that had lit the
emitter at `17:09:01` two hours earlier. Same trigger, same socket, no camera.

Zero `/dev/video*`. The old binary opened the device **1–2 s after `Started`** — model load and
the 39 MB SHA256 come first, then the warm thread — and then held it until the process exited.
The `336 ms` in the journal line is the burst *span*, not the time to the open, and not the
time the emitter was on: that was the whole idle window.

### The residual, and it is deliberate

The camera is still held for the rest of the idle window **after a genuine unlock**, because
`Session` keeps the `Capture` so a retry does not re-open the device. Closing it between
bursts would put a ~400 ms re-open in front of every lock-screen retry, against a 400 ms p95
budget. Left as it is, on the operator's call — it is now bounded by an actual authentication
rather than by cron's schedule.

### The lesson

**A daemon that starts itself has no idea why it was started, and eager work in `main` is
work done for the worst caller.** [B25](#b25) was the same fact seen from the client side — a
`connect()` is a start — and it was fixed there by not connecting. Here the connect is
legitimate and the fix is on the other end: do nothing on startup that a request has not
asked for. The property is now asserted rather than reasoned about, in
`tools/activation-try.sh` PART 2: after a `status` request, the daemon holds **zero**
`/dev/video*` fds. The harness header had *claimed* since the day it was written that parts
1–4 "never open the device" — it was describing the requests, and the daemon was opening the
device behind its back.

---

## B28 — `bench`'s first sample is a process start, and at n=20 it *is* the p99 {#b28}

**Severity: high**, and entirely latent: `bench` is the one subcommand nothing had ever run,
so its first output was going to be believed.

### What happened

The loop measured every iteration and discarded none:

```rust
for i in 1..=runs {
    let answer = ask(settings, Op::Auth, &user)?;
    samples.push(answer.round_trip.as_millis());
```

Under socket activation the first `ask` is what **starts the daemon**, and `client::ask`
begins timing before `connect`. So sample 1 contains a model load and a SHA256 of 39 MB
(377 ms), a V4L2 open, and the startup warm burst that `authenticate` then queues behind on
the session mutex — none of which is an authentication.

The reference machine's own audit log measured the gap during an ordinary enrolment:

```text
enrol pose 1   elapsed=1783        <- the request that started the daemon
enrol pose 2   elapsed=421
enrol pose 3   elapsed=397
```

**4.3×.** Sorted into 20 samples that lands last, and `percentile` takes
`ceil(0.99 × 20) = 20` → `samples[19]`, the maximum. So `p99` and `max` were *exactly and
only* the cost of starting a process, printed as the tail latency of a face unlock — into
[06](06-performance-budget.md), which nominates this command as the source of that number.

The footnote made it worse rather than better: *"p99 here is the slowest sample, not an
estimate of one"* is true, and reads as a caveat about small `n` rather than a warning that
the slowest sample is a one-off.

### The fix

One unmeasured authentication before the loop, announced so nobody wonders what it was.

### The lesson

**This is [B15](#b15) again**, which benchmarked inside the CPU's boost window and reported
half the true cost; the fix there was a warm-up too. What changed is the size and the source:
socket activation made the cold sample ~4× rather than ~1.5×, and made it the *normal* first
unlock rather than an artefact of measuring. A benchmark's first iteration is a different
operation from its others, and that is true again every time the process lifecycle changes
underneath it.

---

## B27 — One refused enrolment pose disabled idle-exit permanently {#b27}

**Severity: high.** It defeats the exit criterion of the phase that introduced it — "zero RSS
when idle" — and it does so by leaving a camera streaming indefinitely.

### What happened

`serve::run` polls with a timeout and, on a timeout, exits only if nothing is half-finished:

```rust
Wait::Idle if daemon.may_idle_exit() => { /* exit */ }
Wait::Idle => {}                              // <- "not now, carry on serving"
```

`accept` **blocks**. So "carry on serving" fell into a blocking accept and the loop never
reached the poll again. The one poll that could have observed the enrolment expiring had
already happened — 30 s in, against a 60 s `ENROL_EXPIRY` — and there was never a second one.
A daemon that could not exit once could not exit ever, and sat holding `/dev/video2` open
until some unrelated process happened to connect.

**One refused pose was enough to reach it.** `Daemon::enrol` banks a `Pending` on its *failure*
path as well, to preserve poses already given when a later one fails — so a single
`Environment` refusal in a bright room creates a 0-pose `Pending` and pins the daemon. That is
not a corner case on this hardware: [B18](#b18) means every enrolment attempt before dark ends
that way, and the operator made nine of them in one morning.

The daemon's own test asserted `may_exit` returns true for an expired `Pending`. It does.
Nothing ever asked it a second time.

### The fix, and the shape that hid it

`Wait::Idle => continue`. One word — but the reason it shipped is that **the defect was
invisible in the control flow**: an empty match arm reads as "do nothing", and what it
actually meant was "fall into a blocking syscall". So the decision is now a function of its
two inputs, returning `Accept` / `Poll` / `Exit`, and every combination is asserted —
including that an `EINTR` must not be read as an idle timeout.

### The lesson

**An empty match arm is not "do nothing", it is "do whatever comes next".** When what comes
next blocks, a fall-through is a state change. Any loop that mixes a timeout with a blocking
call needs its control flow to be a value rather than a shape, because a shape cannot be
tested and this one had three tests around it that all passed.

---

## B26 — The installer wrote a placeholder config, and P5-e made it permanent {#b26}

**Severity: medium**, and it is the second half of the same afternoon as [B25](#b25) — both
were found by one operator running one command on a freshly installed machine.

### What happened

`tools/install-local.sh` created `/etc/faciolock/faciolockd.toml` containing a single comment
line, on the reasonable-sounding theory that an installed package should leave a config behind
for the operator to edit.

`probe::write_config` opens with `create_new(true)`. That is [P5-e](README.md#decisions-taken):
`diagnose --write` must never overwrite an operator's config, so that a diagnostic run in a
bright room cannot rewrite a working machine. The check is on the file's *existence*, because
existence is the only thing a filesystem will tell you cheaply.

So the placeholder was indistinguishable from a real config, and the one command with
something to write could only ever answer `already exists and will not be overwritten`. On a
fresh install the machine had no `device` and no `tier`, and no supported way to acquire them.

### The fix, and why it is a deletion

The installer now creates `/etc/faciolock/` and nothing inside it. Both readers already
document a missing file as the correct fresh-install state:

```rust
// Config::load
// A **missing** file is a fresh install and defaults are the right answer.
// Settings::load
// A missing file is a fresh install and the defaults are right.
```

Two components had already agreed on the convention and the installer disagreed with both.

### The lesson

**A placeholder is a value.** It looked like an empty gesture — a comment, no settings, nothing
configured — but `create_new` does not read the file, and to the one guard that mattered the
placeholder was a fully-configured machine. Anywhere "absent" is a meaningful state, writing a
file to be helpful destroys the state that meant *ask me later*.

---

## B25 — `diagnose` started the daemon that then took the camera it was measuring {#b25}

**Severity: high.** Not for what it damages — nothing — but because it made the one command a
new user runs first fail every time, on a freshly installed machine, with a message that sent
them looking for a process nobody had started.

### What happened

Phase 6 made the daemon socket-activated. `faciolockctl diagnose` had, since Phase 5, opened
with a courtesy line:

```rust
match client::ask(&settings.socket, Op::Status, "root", Duration::from_secs(2)) {
    Ok(_) => println!("daemon      answering"),
```

Before activation that `connect()` was inert: on a machine where no daemon was running it
failed, printed the reason, and the camera stayed free. Afterwards the same call **starts the
daemon**, and `Daemon::serve` opens the camera unconditionally on startup. Eleven lines later
`diagnose` opened `/dev/video2` for its own 30-frame illumination probe and got `EBUSY`.

The operator's journal is the whole bug in five lines:

```text
00:46:49  Started faciolock.service          <- the diagnose poke
00:46:49  listening on /run/faciolock.sock (socket-activated)
00:46:51  camera warm on .../video2          <- the daemon took the device
          ...                                   diagnose then: "camera is in use by another process"
00:47:19  idle for 30 s, exiting
```

Deterministic, and self-perpetuating: every retry re-triggered it. Two activations are in the
journal, one per run, because the operator quite reasonably tried again.

### The fix

`diagnose` no longer connects at all. `daemon_line()` stats the socket path and reports what a
`stat` can honestly support — there is a socket, or there is not.

The line that was lost was worth less than it looked. **Under socket activation "daemon
answering" is close to a tautology**: connecting is what makes it true. What a connect could
still tell you — whether the daemon would *start cleanly* — is real, but it cannot be had
without starting it, and `systemctl status` and the journal already answer it for free.

The property is now held by a test that binds a real listener and `poll`s it afterwards to
assert nothing ever arrived.

### The lesson

**Phase 6 changed what `connect()` means, and nothing audited the callers.** Socket activation
converts a read-only probe into a service start, silently, everywhere in the codebase at once.
The daemon's own code was reviewed carefully for it — `serve::inherited` refuses a malformed
activation, the socket is not unlinked when systemd owns it — and the *clients* were not
reviewed at all.

The general form: when a primitive gains a side effect, the change is not where the primitive
is defined, it is at every call site. Grep for the call sites.

---

## B24 — A flat printed face passes every liveness check {#b24}

**Severity: critical.** This is the answer to Phase 0's S5, the one genuinely unproven claim
in the project, and it is the answer the plan was written to allow but hoped not to get.

### What happened

A magazine cover with a printed face, pressed flat against a rigid backing and held at ~40 cm,
was recorded on 2026-08-19 at 23:45 IST along with a same-session live control. Both went
through `liveness_corpus`:

```text
capture                     pairs   face dark(worst)   D(worst)   falloff    motion  verdict
flat_printed_face              42    yes     2.75e-6    49.7947    0.2733    0.0000  allow
nonflat_printed_face_curved    42    yes     1.73e-6    41.5470    0.1709    0.0000  allow
live_bright_control            42    yes     1.83e-5    54.0582    0.3281    0.0000  allow
```

The print is **detected** by YuNet — so the CMYK-transparent-in-NIR worry does not
materialise on this stock — and it then clears every enforced check: `dark_ratio_max` by
three orders of magnitude, `response_min` by ten times, `falloff_cv_min` by five times,
`motion_max` exactly. There is no check left for it to fail. Curving the same cover over a
mug does not change the verdict either.

**What this does not mean.** Nothing unlocked. The magazine's face is not the enrolled user,
matching still gates the pipeline, and there is no enrolment on this machine to match
against. The claim is narrower and worse: **liveness contributes nothing against a print.**
Whatever rejects a printed attack today, it is not the subsystem whose job that is.

### `falloff_cv` cannot be repaired by moving its floor

| class | n | min CV | max CV |
|---|---|---|---|
| live (nocturnal) | 8 | 0.2661 | 0.3481 |
| live (daylit) | 4 | 0.3071 | 0.3847 |
| **flat print** | 1 | **0.2733** | **0.2733** |

The print sits **above** the worst live face. `worst live 0.2661, best flat 0.2733,
margin -0.0071` — the ranges interleave, so no threshold exists, at any value, that admits
every live capture and rejects the print. The enforced floor of 0.05 was always documented as
a degenerate-flatness guard claiming nothing about real prints ([12](12-falloff-calibration.md));
this is the measurement that shows the gap between what it guards and what it needs to.

[B21](#b21) predicted this direction before the recording existed: a printed face carries
albedo structure — dark brows, bright forehead, shadowed sockets — and albedo enters the
ON−OFF difference exactly like geometry, because the check cannot tell them apart. The
prediction was that a real print would read *higher* than the blank page's 0.2486 median,
closer to live. It reads 0.2733, and "closer to live" turned out to mean past it.

### The lead: real, and far thinner than it first measured

The `ctr/edge` column, which nothing enforces and the corpus test only prints, appeared to
separate cleanly:

| class | ctr/edge, worst of 41 pairs |
|---|---|
| live faces (12 captures, both regimes) | 1.446 – 1.774 |
| flat printed face | 0.894 |
| printed face curved over a mug | 0.876 |
| flat print + IR floodlight | 0.918 |

**Two corrections to this entry's own earlier drafts**, both worth keeping visible because they
are opposite failure modes and the second was the more expensive:

*First draft* quoted the live band as 1.446–1.954 over 11 captures. 1.954 and 1.602 are
`attack_floodlight_daylight` and `attack_floodlight_room` — *attack* rows, pooled into the live
class by reading down a column. The honest band is 1.446–1.774 over 12 captures, and the error
inflated the very gap the section was arguing for.

*Second draft* then over-corrected, declaring the statistic refuted and citing four mechanisms —
that the live signal was the box border sampling dark room, that shrinking the box collapsed
live into the printed band, that a blank page reached 2.08, and that the print's reading was a
low-percentile draw from its own placement distribution. **All four were checked and all four
failed.** They are recorded here as retractions rather than deleted, because each is a plausible
story that a careful reader would otherwise re-invent:

- **"The edge ring is background."** It is majority on-face. For `live_bright_control_20260819`,
  mean `D` is 163.6 in the centre cells, 100.2 in the 20-cell edge ring, 43.7 in the annulus
  just *outside* the box, and 16.1 in the far field. The ring sits 2.3× above its surroundings —
  it is cheek and jaw, not room.
- **"Shrink the box and live collapses into the print band."** The comparison was unmatched —
  live at 0.5 scale against prints at 1.0. At matched scale the separation survives at every
  scale the pipeline could operate at; the first inversion is at s=0.50 and it is driven by a
  *live* capture falling below the print band, which is a false-reject, not a spoof-accept. And
  there is no shrink: `post_box` takes `d.bounds` straight through with no rescaling, so s=0.5
  is not an operating point and not attacker-reachable. The discriminator was wrong too — the
  repo's own synthetic dome and a modelled Lambertian hemisphere both collapse under the same
  shrink.
- **"A blank page reaches 2.08."** No provenance. That figure appears in no test output and no
  wiki page; `flat_printed_page` takes the placement-search branch and the falloff test prints
  no `ctr/edge` for it at all. The flat printed *face* reaches 0.00% of 4,482 placements above
  1.35.
- **"0.894 is an 18th-percentile draw, so it is placement bias."** The symmetry control kills
  it: run the identical sweep on live captures and their placement medians are also ~0.91–1.07,
  with their *detected* readings at the **92.9th–99.7th percentile**. Prints land low and faces
  land high precisely because YuNet puts the box on the subject. The percentile is the
  measurement, not the confound.

So the mechanism is not what the second draft claimed. What is actually wrong with the finding
is narrower, and it is enough:

**1. The 0.53 gap is an artifact of a worst-of-41-pairs statistic, and the pipeline never sees
41 pairs.** `min_pairs = 2`, the daemon needs `min_pairs * 2` frames, and a burst is
`burst_frames = 6` — three ON/OFF pairs, ~0.25 s. Per-pair on the detected box:

```text
                              n    min      med      p90      max
flat_printed_face            41    0.894    1.280    1.418    1.443
live_lit_frontal (weakest)   41    1.446    1.478      -        -
```

The print's *maximum* pair sits at 1.443 against the weakest live capture's *minimum* of 1.446.
**The real margin is ~0.2%, not 0.53** — and the 0.894 headline is the single worst pair of a
7.4 s hand-held capture, a statistic the running system would never compute.

**2. The corpus is n=1 on both sides, and only one of those was previously written down.** One
magazine cover — three takes inside 82 seconds is one object, not three samples. And on the live
side, **one subject**: all 12 live captures are the operator, there is no `subject` field in any
sidecar, and no second person appears anywhere in `corpus/`. A floor calibrated from one head
risks locking out real users as much as admitting attackers, and nothing in this project had
flagged that until now.

**3. Zero daylight prints.** Every print capture reads pedestal 0.000. The only flat object at
nonzero ambient is `flat_printed_page` at 7.898 — blank stock, no detected box, and 6.5× below
the daylight regime that motivated the errand (`live_daylight_frontal` sits at 51.7). Whatever
`ctr/edge` does at pedestal 50 is unmeasured, and the ambient-cancellation argument for it is
the *same* derivation `falloff_cv` had — which was just refuted.

**4. It is not face-specific.** A bare ceramic mug reads **1.800** — above every live face in the
corpus. Whatever the statistic responds to, "is alive" is not it; convex geometry is enough.

**Verdict: no threshold, enforced or advisory.** Not because the statistic is refuted — it is
not, and the placement symmetry control is genuine evidence *for* it — but because a ~0.2%
margin at the real burst length, from one print and one face, in one ambient regime, is not a
number anyone should gate authentication on. An advisory threshold would still put a value in
the config and a row in the report that reads as progress.

> **Provenance, kept because [B19](#b19)'s lesson was about exactly this.** **Observed** here:
> the live band and the two mis-pooled attack rows (the corpus test's own output), `min_pairs`
> and `burst_frames = 6` (the configs), the mug at 1.800 (the same output), and the three code
> defects — the new unit test was run against the pre-fix function and fails on it, `left: inf,
> right: 0.0`, "a dead border must not read as live". **Re-derived, not re-run by hand:** the
> per-pair distributions, the placement percentiles, and the edge-ring annulus figures, each
> produced by independent throwaway harnesses that reproduce the shipped table to 4 dp. That is
> enough to decline a threshold. It would not be enough to set one, and the retractions above
> are what that distinction costs when it is skipped.

### What was fixed, since the statistic stays as a reported column

Auditing it for promotion found three defects that would have mattered the moment it carried a
threshold, all of them in the *passing* direction under the `Fails::Below` wiring it would need:

- `INFINITY` was returned when the border mean was ~0 — which passes any minimum. The border of
  a presented object is exactly what an attacker chooses, so a matte-black IR-absorbing surround
  is the input that reaches it.
- A uniformly negative field (ON darker than OFF) returned a large **positive** ratio, also
  passing. `falloff_cv` has guarded its analogous case since B18; the guard never reached here.
- A slice shorter than 36 cells **panicked**, and a panic on the daemon's path is a refused
  unlock.

All three now return 0.0, the failing value, held by
`centre_edge_fails_closed_on_every_degenerate_input`. No corpus placement triggers any of them,
which made them latent rather than benign. Note the contrast with `dark_ratio`, which returns
`INFINITY` deliberately and correctly — it is `Fails::Above`, so the *direction* of the check
decides what a degenerate input should return, not the arithmetic.

### What would actually settle it

The second draft of this section proposed a black-surround / white-surround print pair, on the
theory that the live signal came from the box border sampling dark room. That theory is
retracted above, so the experiment it justified is no longer the pointed one. What the corpus
actually lacks, in priority order:

1. **A second live subject.** This is now the cheapest and most glaring gap: every live number
   in this project comes from one person's head. It needs no printer and no errand, and it
   bounds the false-reject side that nothing has ever tested.
2. **A second print object**, different stock and tone, so the print side stops being n=1. Held
   **propped, not hand-held** — the 0.894 headline came from a sheet drifting over 7.4 s — and
   sized so its detected box is close to the live control's, which `corpus/README.md` rule 3
   already required and the 2026-08-19 set broke.
3. **Prints in daylight**, at a pedestal near the 51.7 the live corpus reaches. Every print
   number here is nocturnal, and the ambient-invariance argument for `ctr/edge` is the same one
   `falloff_cv` had before it was refuted.

And one change that costs no recording at all: **the corpus test should report the per-pair and
3-pair-burst distribution, not the worst of 41.** The entire apparent 0.53 margin was an artifact
of a statistic the running system never computes, and no future session should be able to
reproduce that mistake by reading the same column.


### Why this is worth the entry it is getting

S5 has been open since day one and was the item the roadmap called "the one genuinely
unproven claim in the project". It closed for the price of one magazine cover and eight
minutes of recording, on the branch that says the current design is insufficient.

That is the cheap version of this discovery. The expensive version is a security review after
a PPA release, or a user with a printed photo. The whole point of Phase 0 existing was to buy
the cheap version, and here it did — several phases later than planned, which is its own
lesson: this recording was deferred on 2026-08-04 as a risk-register item and every week it
stayed deferred was a week the falloff design looked settled.

---

## B23 — A security control implemented as global process state {#b23}

**Severity: medium**, and the severity is the interesting part: the *socket* was always
correct, on every run, in production and in tests. What broke was a directory belonging to
something else entirely.

### What happened

Phase 2 wanted the socket at 0600 from the instant it exists, because `chmod` after `bind`
leaves a window at the process umask. The fix was to set the umask around the `bind`:

```rust
let previous = unsafe { libc::umask(0o177) };
let bound = unsafe { libc::bind(...) };
unsafe { libc::umask(previous) };
```

That reasoning is right and the window it closes is real. But `umask` is not a property of
the call, it is a property of the **process**, and for the duration of that `bind` every
other thread creating a file or a directory inherits it too.

Phase 3 added a second test that calls `listen()`. `cargo test` runs tests in parallel
threads, so `store::save`'s `create_dir_all` landed inside the window and the template
directory was created at `0777 & ~0177` = `drw-------`. A directory without its execute bit
cannot be opened, so the next line failed with:

```text
/tmp/faciolockd-store-test/csr.tpl: Permission denied (os error 13)
```

Two store tests failed, in a file nobody had touched, naming a permission error on a
directory the test had just created itself. The suite had passed minutes earlier with the
same code — one `listen()` call was a narrow enough window to usually win the race.

### The fix

`bind` to a private staging name, `chmod` it, `listen`, then `rename` into place. `rename`
is atomic, so the real path is either absent or a 0600 listening socket, with no third
state — a stronger guarantee than the umask version gave, since the staging inode is never
at the real path at all. No global state is touched, so nothing else in the process can
observe it. It also subsumes the `remove_file` that used to be needed for a stale socket
from a crashed run: the rename replaces it, rather than an unlink that something could win.

### Why this is worth a numbered entry

It never affected the daemon. `listen()` is called once, at startup, before any thread
exists, so the socket was 0600 and the umask was restored microseconds later. Every
production run was correct.

**The bug is that the correctness was ambient rather than local.** The single-threaded
caller was load-bearing and nothing said so; the guarantee held because of a fact about the
program elsewhere. That is the class of thing that survives every review — the code is
right, the comment explaining it is right — and then breaks when the surrounding program
changes in a way that has nothing to do with sockets. Here the change was a *test*, which
is the cheapest possible way to find out.

The lesson is narrower than "avoid global state": a control that borrows a process-wide
setting to achieve a local guarantee has a scope its author did not choose. When a
file-scoped mechanism exists — `rename`, `O_TMPFILE`, `fchmod` before the name is
published — it is the one to reach for, even at a few more lines, because its blast radius
is the thing it is about.

---

## B22 — The AGC settle frames were discarded twice {#b22}

**Severity: high.** Every live authentication would have returned
`Unusable(TooFewFrames { got: 1 })`. Not "sometimes", not "in daylight" — always, for
everyone. It survived 119 passing tests and was found by the **first** end-to-end run
against the real camera, which is the entire reason Phase 2's exit criterion 4 exists.

### What happened

Two layers each discard the frames the sensor produces while its AGC converges (wiki/10
Probe 3), and neither knew about the other:

```text
faciolock-camera   Capture::start(warmup)              default 5, at stream open
faciolock-vision   Config::agc_settle_frames           default 5, per burst
```

The daemon asked for a 6-frame burst. The camera warmed the stream, discarding 5 frames
once at startup, and handed over 6 fresh ones. `Burst::from_frames` then took
`&frames[agc_settle_frames..]` and threw 5 of those 6 away. One frame is not a pair, so the
burst never formed.

Both defaults are correct for the caller each was written for, and that is the whole bug.
`agc_settle_frames = 5` is right for the corpus, which arrives as 90-frame `.gray`
recordings whose head is genuinely unsettled. `warmup = 5` is right for a warm streaming
session. Composing them was nobody's default.

### The fix

The daemon sets `agc_settle_frames = 0` explicitly, with the reason written at the
assignment: for live capture the discard belongs to the camera layer, which already did it
at `Capture::start` and which additionally flushes every queued frame before each burst
(**P2-c**, fresh frames only). So every frame the daemon hands to the vision crate is
already post-warmup and post-flush, and a second discard has nothing left to discard.

A startup guard was added beside it, because the failure mode was silent rather than loud:
a `burst_frames` too small to yield `min_pairs` now refuses to start, instead of binding the
socket and answering `Unavailable` forever.

### Why the tests could not have caught it

Every corpus test feeds `Burst::from_planar` a 90-frame recording, where discarding 5 twice
still leaves 80. The bug is only reachable when the burst is short — which is to say, only
when the frames come from a camera rather than from a file. The vision crate's headless
replayability is the property that makes its security claims falsifiable (wiki/04), and this
is its exact cost: the seam between two correct crates has no corpus.

### The lesson

**A default is a claim about a caller.** Two crates each owning a settle-frame default is
not duplication to deduplicate; it is two different questions with the same name. The bug
appears at the moment one crate becomes the other's caller, and the fix is not to delete
either default but to make the composing layer say which one applies and why.

It is [B19](#b19)'s lesson at a different seam: there, a sidecar stated a parameter it had
never observed. Here, a pipeline applied a correction that had already been applied. Both
are a number carried across a boundary without anyone asking what it meant on the other
side.

---

## B21 — A liveness threshold calibrated by choosing its own ROI {#b21}

**Severity: high — caught before it was believed, which is the only reason it is not
critical.** For about twenty minutes this test reported that falloff **separates** a flat
surface from a live face by 2.45×. It does not. The separation was manufactured by the
rule that chose where to measure.

### How it happened

Falloff needs a face box. A flat presentation has no face, so YuNet returns nothing and the
ROI has to come from somewhere else. Three rules were tried, in this order:

1. **Fixed box at the median live position** (108×138 at 296,111). On `flat_printed_page` it
   straddled the top edge of the notebook — two thirds page, one third dark background. The
   bright/dark step inside the ROI read **CV 0.4489, above every live face.** Nonsense, and
   visibly so: a flat surface cannot beat a face on geometry, because across a head-sized
   region at 40 cm the corners are only ~2.5% further from the emitter, so centre/edge should
   be ≈1.05 and it read 1.314.
2. **Search for the box whose weakest cell is strongest** — the obvious "is it on the object"
   rule. Gave **CV 0.1085 against worst live 0.2661**, a clean 2.45× margin, and the test
   printed `SEPARATED`.
3. **Every valid on-object placement, as a distribution.** `0.0361 … 1.7116`, median
   **0.2486** — against a worst live of 0.2661.

Rule 2 is rule 3's minimum. Maximising the weakest cell selects the most *uniformly lit*
patch, which is precisely the patch with the lowest CV. The rule intended to answer "is the
box on the object?" silently also answered "which part of the object flatters the defence?"

The spread is not a detail. **0.036 to 1.71 is wider than the entire live band**, so under
rule 2 the ROI choice, not the physics, decided the result.

### Why no ROI rule fixes this

The real pipeline never chooses an ROI. **YuNet returns the box** — and against a genuine
print attack it returns the box *on the printed face*. So the only number that decides
anything is falloff over a **detected** box on a **printed face**, and no amount of cleverness
about where to put a rectangle on a blank page produces it.

A blank page is not a conservative stand-in either. It is an **optimistic** one: a printed
face carries albedo structure — dark brows, bright forehead, shadowed sockets — and albedo
variation enters the ON−OFF difference exactly like geometric variation does, because the
check cannot tell them apart. A real print should therefore read *higher* than a blank page,
i.e. closer to live.

That last point is the uncomfortable one, and it generalises beyond this bug: **falloff
assumes the only thing modulating the emitter's response across the face box is distance.**
Albedo modulates it too, and a photograph of a face is by construction a surface whose albedo
pattern imitates a face.

### Consequences

- **The falloff floor stays uncalibrated**, and Phase 0's S5 stays open. The 2026-08-09
  session did not close it.
- **The test no longer emits a verdict from a chosen ROI.** It prints the placement
  distribution and refuses. `SEPARATED` is reachable only from detected boxes.
- **Falloff's premise needs stating in [05](05-vision-pipeline.md)** — that it reads geometry
  — alongside the fact that albedo is a confound it cannot separate.

### The lesson

The bug was not the ROI rule. It was that **a free parameter was chosen after seeing which
value made the answer come out well**, without anyone deciding that was what was happening.
Rule 2 was picked because it sounded principled, and it *is* principled for the question it
was written for. It just also silently answered the question being measured.

Whenever a measurement has a knob the measurement itself does not fix, sweep the knob and
publish the range before quoting any single value from it. Had the sweep been run first, no
version of this would have been believed for a minute — it took ten lines and it is now
what the test does by default.

---

## B20 — The ambient gate is opened by the attacker, not closed {#b20}

**Severity: critical.** [B18](#b18) placed the ambient gate on the argument that the floor is
the one quantity an attacker can only move in the *safe* direction:

> "The floor is the only candidate an attacker can move in one direction only: raising
> ambient raises the floor, which **closes** the gate."

**That is refuted by measurement.** Presenting a bright object 40 cm from the lens collapses
the measured floor, which **opens** the gate.

### The evidence, from one session

2026-08-09, one room, one lighting description, six takes over nine minutes:

```text
capture                      floor    whole:amp   verdict
live_bright_control          28.221       64.20   GATED  (AmbientTooBright)
nonflat_wrapper_crinkled      0.529       38.92   GATED
attack_wrapper_floodlight     0.111       38.34   GATED
nonflat_card_bowed            0.082       51.75   GATED
nonflat_mug_ceramic           0.013       39.59   passed the gate -> REJECT (dark-frame)
```

The operator's own face reads **28.221 and is refused**. A ceramic mug held where the face
was reads **0.013 — a factor of 2,170 lower — and passes**. Same room, same minute, same
sidecar. The honest user cannot get under the gate; the presented object gets under it
easily, because *the object is what sets the floor*.

### Why: exposure, not gain, and it is measurable

The mechanism is visible in the asymmetry between the two columns:

```text
                    live -> mug     ratio
ambient floor    28.221 -> 0.013   2,170x  collapses
strobe amplitude  64.20 -> 39.59     1.6x  barely moves
```

A **gain** reduction scales signal and pedestal together, so both columns would fall by the
same factor. They do not. A shorter **exposure** scales the ambient contribution with
integration time while still catching the emitter pulse in full — which produces exactly
this: pedestal down three orders of magnitude, amplitude down 1.6x. The sensor AGC is
responding to a bright near object by shortening exposure, and the ambient pedestal goes
with it.

So `pedestal` was never measuring the room. It measures *what the AGC settled on*, which is
dominated by whatever is nearest and brightest — i.e. by the attacker's prop.

### What held, and it should be said as plainly as what broke

Nothing was falsely accepted. Layered defence did its job:

- The mug was the only capture through the gate, and `dark_ratio_max` rejected it — at
  **1.79e-3 against a 1e-3 threshold, a 1.8x margin.** Thin, and now the load-bearing layer
  for this whole class. Worth recording as its own risk.
- The floodlight take was **gated at 0.111** — injected IR *raised* the floor and closed the
  gate, exactly as B18 designed. The one-direction claim holds for injected IR; it fails for
  presented objects, which B18 never considered.

### Consequences

1. **The gate cannot stay keyed on a quantity the attacker sets.** Either it keys on
   something an attacker cannot pull down, or the gate stops being a security control and
   becomes a UX hint, with the real work done by the per-pair checks.
2. **`Verdict::Environment`'s failure-budget exemption is now a probing oracle.** B18 exempted
   `Environment` so weather could not disable face unlock. Combined with an attacker-lowerable
   floor, that is unmetered retries against a control the attacker steers. The exemption needs
   re-deciding before Phase 4 builds the rate limiter on it.
3. **This is measurable, not just arguable.** `MSXU_CONTROL_METADATA` (selector 0x09) is
   supported on this camera ([10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode)) and the kernel exposes
   `UVCM` on `/dev/video3`. If it carries `KSCAMERA_METADATA_CAPTURESTATS` exposure time, the
   AGC hypothesis stops being an inference from two columns and becomes a per-frame reading.
4. **It points the same way as Probe 6.** Short exposure suppressing ambient while preserving
   the emitter pulse is precisely the effect MSXU face-auth mode is expected to produce
   deliberately. This session got there by accident, driven by the AGC. That is encouraging
   for the daylight question and it is **not** a substitute for running the experiment.

### The lesson

B18's gate was reasoned about as though the scene were a fixed property of the room and the
attacker only added light to it. But the attacker controls the largest, nearest object in
frame, and on a camera with automatic exposure and no manual override that means the attacker
controls the exposure, and therefore the pedestal, and therefore the gate. **A threshold is
only as trustworthy as the adversary's inability to move its input** — and "ambient light" was
never an input the adversary lacked a handle on.

---

## B19 — The corpus recorded the frame rate it asked for {#b19}

**Severity: medium.** Found alongside [B18](#b18) and worth its own entry, because it is
the reason the latency budget was wrong for four days without anybody being able to notice.

`tools/record.sh` wrote `"fps": 30` into every sidecar. 30 is the value it passed to
`ffmpeg`, not a value it observed. `VIDIOC_G_PARM` also reports 30, because that too is the
*configured* rate. So every tool in the project agreed on a number that nothing had
measured.

Measured from kernel buffer timestamps on 2026-08-06:

```
p50 inter-frame interval   68.0 ms   (min 63.8, max 68.1 — steady, not jitter)
delivered rate             14.7 fps
sidecar-recorded rate      30 fps
```

`record.sh` now times the capture and writes `fps_delivered`, `elapsed_s`, and a
free-text `ambient` field, plus `recorded_local` so the time of day is legible without
timezone arithmetic. The first two captures taken with it read **12.11 fps**.

### What it cost

[06](06-performance-budget.md) budgets the frame burst at 130 ms for 6 frames, which
assumed 30 fps. Two things follow, and the second is worse than the first:

1. At the delivered rate a 6-frame burst spans **~340–410 ms** — the entire 400 ms p95
   target, before a single inference runs.
2. **The 130 ms figure was never achievable even under its own assumption.** Six frames at
   33.3 ms spacing span ≥167 ms first-to-last. The arithmetic was wrong independently of
   the hardware.

### The lesson, which is [B4](#b4)'s in a new place

B4 was a capture with *no* sidecar. This is a capture with a sidecar that **states a
parameter it never checked**, which is worse: the missing sidecar announces its absence,
while the wrong one is quoted with confidence. Anything a sidecar records as a measurement
must be observed after the fact, never copied from the request.

---

## B18 — Every strobe statistic is a ratio, and ambient light saturates a ratio {#b18}

**Severity: critical.** A *design* bug, found the first time the camera was read outside
the hours the corpus happened to be recorded in. It made the reference machine unable to
authenticate at all in daylight, and it invalidated the one liveness threshold the project
had ever called verified.

### What happened

Phase 2's capture layer was run against the real camera at 09:16 IST. The machine
classified as `ir-constant`, not `ir-strobe`, so no burst formed and every authentication
would have been refused.

Confirmed across three independent code paths — the new Rust capture layer, a raw `ffmpeg
signalstats` run using [10/Probe 3](10-hardware-probe.md#probe-3--per-frame-mean-luma-the-decisive-test)'s
own command, and `spikes/probe`, the unchanged Phase 0 binary that produced Probe 3's
numbers. `dpkg.log` shows no kernel, firmware or `uvcvideo` change since the corpus was
recorded, so this is not a driver regression.

### The emitter never stopped

This is the part that was misdiagnosed for the first hour, and getting it wrong would have
led to a redesign that was not needed.

The emitter fires **every frame, perfectly regularly, with no missed beats**, in daylight
exactly as at night. What collapsed is the *statistic*:

```text
                     ambient floor   amplitude   same-phase noise      SNR   ratio test
live, nocturnal ×5          0.0002       54–81        0.005–0.047   1,525+     Strobing
live, daylight               51.74       32.99              0.060      549   Constant  <-
floodlight attack            0.0004       53.11              0.999       53     Strobing
```

`classify` asked `hi > lo * 10` — a **ratio**. Ambient near-infrared adds a pedestal to
both phases, and a ratio saturates against a pedestal: 15,000:1 at night becomes 1.15:1 at
9am while the emitter's actual contribution barely moves. Both frames carry a full
structured scene (sd ≈ 53, min 12, max 253), so this is ambient light forming an image, not
gain-amplified noise.

**Every corpus recording was made after dark** — `20260803T183445Z` is 00:04 IST,
`20260804T145045Z` is 20:20 IST — and nothing in the sidecars recorded that, so the entire
corpus described one lighting condition without saying so. The variable was invisible
because it was constant. [10](10-hardware-probe.md)'s "Still to probe" list had predicted
exactly this measurement and it arrived before the probe was run.

### What was fixed

`frame::classify` now measures a **difference**, which a pedestal cannot touch: the median
step between neighbouring frames over the median step between frames of the *same* phase —
a noise floor measured on the capture itself, since Probe 3b showed that floor move tenfold
between two runs on one machine.

Phase assignment changed with it, from a split of the global range to a comparison against
**immediate neighbours**. The global split held only while a capture's drift stayed under
the emitter's amplitude; in daylight the ON series climbed 105 → 116 while OFF climbed
92 → 102, leaving 3.5 levels of headroom on a 14-level amplitude. A slightly longer capture
would have put a late OFF frame above an early ON one and mislabelled both silently.

The daylight capture now classifies as `Strobing` and forms a burst.

### What is still open, and it is the important half

**`dark_ratio_max` is also a ratio** — `mean(OFF)/mean(ON) > 1e-3 → reject` — and it is the
project's only `Enforced`, corpus-verified threshold. The daylight capture reads **0.664
against a threshold of 0.001, with all 42 pairs failing.** Honest captures in daylight
still cannot authenticate; they are now refused one layer later, by liveness rather than by
tier conformance.

Three replacements were measured, all kept per-pair so [B9](#b9)'s conjunctive rule
survives. A burst-level SNR floor was rejected without measuring: it is an average, and B9
is precisely the finding that averaging liveness across a burst is a 1-in-36 bypass.

```text
                          old ratio   A: vs median   B: vs neighbours   C: pair response
live, nocturnal ×6           0.0000         0.0000             0.0000      0.056–0.143
live, daylight               0.7389         0.3095             0.1428           0.0966
floodlight attack, night     0.0867         0.0308             0.0484           0.2979
floodlight attack, DAYLIGHT  0.4907         0.1487             0.0249           0.0781   <-
spoof_phone                  0.0002         0.0001             0.0000           0.2727
```

**All four statistics are refuted, and the daylight attack is what refutes them.**

Against the nocturnal attack alone, C looked promising — it separated by 2.1× and caught
`spoof_phone` for free. The daylight attack recording, made specifically to test it, killed
it: **C reads 0.0781 for the attack against 0.0966 for the honest daylight capture and a
live range of 0.056–0.143.** The attack sits *inside* the live distribution, and below the
honest capture.

It is not one statistic that fails. Three rank the daylight attack as **more live-looking
than a live face**:

```text
                      honest daylight   floodlight in daylight   ranks the attack as
old OFF/ON ratio               0.7389                   0.4907   cleaner
candidate C                    0.0966                   0.0781   cleaner
face-box SNR                    881.7                   1017.3   stronger
```

YuNet also detects a face in the daylight attack (it does not in the nocturnal one), so
detection does not save it either.

### Why, and what actually follows

The emitter's contribution is what carries every one of these statistics. In high ambient
NIR the ambient already dominates, so *adding* a floodlight changes the picture much less
than it does in the dark — and the honest capture and the attacked one converge. There is
no cleverer arrangement of these terms that recovers a separation which is not in the
signal.

### The matched pair (2026-08-08) — separation is dead at ambient 14

The cleanest experiment in the corpus, and the one that should be quoted: same room, same
subject, same position, 90 seconds apart, ambient floors within 1% of each other. One live,
one with an IR remote at the sensor.

```text
                        floor   old ratio     cand A   cand B   cand C   whole SNR   face SNR
live_room_lit           13.91      0.3352     0.1031   0.0269   0.2374       280.5     1535.1
attack_floodlight_room  14.05      0.2523     0.0737   0.0297   0.1071       948.6     2898.1
```

**Every statistic ranks the attack as more live-looking than the live face**, or (candidate
B, 1.1×) fails to separate at all. Earlier evidence for this came from captures at
*different* ambient levels and was open to the objection that the conditions differed. This
pair closes that objection.

The room in question is a large one whose only sunlight source is indirect and about 10 m
away, and the operator reports Windows Hello authenticating there reliably. Two things
follow, and they should not be conflated:

- **Refusing there is correct for this hardware.** At floor 14 there is no measurable
  difference between an attacked capture and an honest one, so any answer other than
  "cannot verify" would be a guess.
- **It is a hardware gap, not a logic gap.** A 940 nm emitter behind a narrow bandpass
  filter — which is what purpose-built face-auth optics use — would see a far lower ambient
  floor in the *same* room, because 940 nm sits inside a solar water-vapour absorption band.
  Windows Hello working there is evidence about its optics, not evidence that strobe
  liveness is recoverable on this sensor.

**Scope consequence.** The ceiling is now bracketed between 0.92 and 13.91 rather than 0.92
and 20.33 — but no placement of the gate inside that bracket recovers a room reading 13.91.
On this hardware Faciolock is a **low-ambient system**: evening, artificial light, curtains
drawn. Daytime rooms with a real window are outside it. That belongs in
[01](01-goals-and-scope.md) as an explicit non-goal, not buried here.

### Correction (2026-08-08, later the same day) — the data held, the inference did not

The matched pair above is good data and its narrow finding stands: **no magnitude
statistic detects IR injection at high ambient.** That is why `dark_ratio_max` cannot be
rescued, and nothing below weakens it.

What does not follow is the next step — "therefore no liveness is possible here, therefore
it is a hardware ceiling, therefore daylight is a non-goal." Three separate problems.

**1. The optics explanation is refuted by its own premise.** It attributed Windows Hello's
success in that room to better optics — 940 nm behind a bandpass filter. It is the *same
laptop*: same sensor, same emitter, same glass. Optics cannot differ from themselves.
Whatever Windows does differently is software operating on the same imagery, which makes
Windows Hello working there *evidence that the problem is solvable on this sensor*, not a
mystery about someone else's hardware.

The obvious candidate was checked and refuted. The IR camera carries a Microsoft Camera
Extension Unit (unit 4, `{0f3f95dc-2632-4c4e-92c9-a04782f43bc8}`) advertising
`MSXU_CONTROL_FACE_AUTHENTICATION`, and Linux had never set it — every capture in the
corpus was taken in `DISABLED`, the general-purpose IR preview mode. Setting
`ALT_FRAME_ILLUM` works and persists, and **does not move the ambient floor**:

```text
DISABLED (baseline)     20.407
ALT_FRAME_ILLUM         18.958   20.693   21.213   21.087
```

Session variance swallows it. This camera already strobed in `DISABLED` and its face-auth
mode does not change the integration time. See [10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode).

**2. Both captures in the matched pair contain a live face.** From the sidecars:
`attack_floodlight_room` is *"IR remote at sensor, face in frame, same position"*;
`attack_floodlight_daylight` is *"IR remote at sensor, face in frame"*. So the comparison
was a live face against a live face plus a contaminant. Ranking one above the other on
"live-lookingness" is measuring the injection, which is real and worth knowing — but it
says nothing about whether a **spoof** would pass, because neither capture holds one. The
corpus contains exactly one spoof (`spoof_phone`, no floodlight), rejected by detection.

**3. One statistic was never in the comparison, and it is the one that survives.** Every
candidate tried — old ratio, A, B, C, whole-frame SNR, face-box SNR — is a **magnitude**.
Magnitudes carry the ambient pedestal, which is precisely why they converge.

`falloff_cv` is not a magnitude. It runs over `on_cells - off_cells`, and a pedestal
present in both phases **cancels exactly** in that subtraction. What survives is the
emitter's inverse-square pattern across the face box — a nose at 38 cm returns more than a
cheek at 45 cm — which is subject *geometry*. Ambient light does not change a subject's
geometry.

It had never been measured in daylight because the ambient gate refuses the burst *before*
liveness runs, so `liveness_corpus` prints `no box` for exactly the three captures that
would answer the question. Measured now, going around the gate
(`falloff_is_computed_on_a_difference_so_ambient_should_cancel`):

```text
capture                     pedestal   falloffCV   ctr/edge
live_lit_angle_left            0.000      0.2661      1.520
live_lit_frontal               0.000      0.2802      1.446
live_lit_frontal               0.000      0.3012      1.469
live_lit_glasses               0.000      0.3066      1.624
live_dark_frontal              0.000      0.3130      1.509
live_night_lamp                0.001      0.3481      1.527
live_daylight_half             0.920      0.3245      1.687
live_room_lit                 13.909      0.3071      1.481   <- the problem room
live_daylight_frontal         51.737      0.3847      1.774   <- 1300x over the gate

nocturnal  n=7   CV 0.2661 - 0.3481
daylit     n=2   CV 0.3071 - 0.3847
```

**The ranges overlap completely.** The problem room reads mid-band. At pedestal 51.7,
where every magnitude statistic is dead, the shape is the strongest in the corpus. Falloff
is ambient-invariant in measurement, not only in derivation.

The gate is therefore placed at the wrong **layer**. It was derived to protect
`dark_ratio_max` — correctly — but it sits in front of the whole burst and takes `falloff`
down with it.

### What this does and does not license

It does **not** license opening the gate. `falloff` enforces only a degenerate-flatness
floor of 0.05 against a live band bottoming at 0.2661, because **no print has ever been
recorded**. Opening the gate today would trade a *measured refusal* for an *unmeasured
acceptance*, which is the wrong direction however the ceiling was misdiagnosed.

And falloff cannot cover for `dark_ratio_max`. The same table, run over the attack
captures, says so plainly:

```text
attack_floodlight_room        14.052      0.3173      1.602
attack_floodlight_daylight    20.329      0.3963      1.954
```

Live-like, as expected — those captures contain live faces. **Falloff measures subject
geometry, so it cannot see injected IR**, and standing `dark_ratio_max` down in high
ambient stands down the only check aimed at injection, in exactly the regime where
injection is cheapest. Whether an attacker can *shape* a difference image — an angled
floodlight across a print, a synchronised strobe — is an open claim, not a solved one.

**So the blocker on daylight was never the sensor. It is the missing negative class** —
Phase 0's S5, deferred since day one and still the project's one genuinely unproven claim.
The recordings that close it, all in the high-ambient room:

- `flat_printed_page` — calibration, and the load-bearing one. ~~**No printer and no printed
  face are required.**~~ **Superseded 2026-08-09 by [B21](#b21): a printed *face* is
  required**, because a faceless object gives YuNet no box and the ROI then has to be
  chosen, over a range wider than the live band. No *printer* is still true, and the face
  still need not be the enrolled user's — any head-sized printed face will do. Falloff
  measures whether the emitter's response across a region is
  flat or domed — surface geometry and IR reflectance — so it does not care what is
  depicted. Worth repeating at night for the cross-check,
  since CV appears to *rise* with ambient (0.3847 at pedestal 51.7 is the corpus maximum),
  the direction that could push a flat surface past a nocturnally-set floor.
- `flat_blank_card` — the same, unprinted. Ink varies IR reflectance and lifts CV, so
  printed matter is the realistic attack and blank card is the floor beneath it. The pair
  brackets the flat class rather than guessing where inside it a real spoof sits.
- `flat_curved_page` — the printed page curved around a mug. The presentation that tries to
  *manufacture* falloff, and the likeliest to approach the live band.
- **`attack_flat_floodlight`** — the flat page *plus* the IR remote. A spoof with injected
  IR, which is the only capture that speaks to standing `dark_ratio_max` down.
- `spoof_tablet_bright` — completeness only. **A screen cannot calibrate falloff:**
  `spoof_phone` already reads `no face`, because the IR camera sees the emitter reflected
  off the glass rather than the displayed image, so screens are refused by detection and
  never reach the check. Useful as the corpus's first daylight screen capture; useless as a
  negative class.

**What a `flat_*` capture can and cannot settle.** It has no face for YuNet to find, so it
is measured over a **fixed** box — the median of this corpus's twelve detected live boxes,
108×138 at (296, 111) — and the test labels the row `FIXED box` so it is never mistaken for
a detected one. That makes flat-vs-live a fair comparison of *surface geometry*, provided
the object is held at the same ~40 cm and fills the frame like a head. It is **not** an
end-to-end attack replay: a real spoof must also pass detection, and these deliberately
cannot. What they calibrate is the flatness floor, which is the one quantity blocking a
rescope. `tools/record-negatives.sh` prompts through the set and the falloff test then
prints the margin — worst live CV against best flat CV — directly.

### Resolved by an environment gate (2026-08-06)

`Config::ambient_floor_max = 0.04`, checked in `Burst::from_frames` **before tier
conformance, liveness or detection**, yielding `BurstError::AmbientTooBright` →
`Verdict::Environment { ambient_floor }` → `Unavailable` on the wire → password prompt.

Three properties it was built to have:

1. **It gates on the ambient floor, not on a ratio or an SNR.** The floor is the only
   candidate an attacker can move in one direction only: raising ambient raises the floor,
   which *closes* the gate. A gate keyed on emitter-to-ambient ratio or SNR puts an
   attacker-raisable term where it would hold the gate **open** — the synchronised-injection
   case B9 never covered.
2. **It runs before any biometric work.** No face is looked for on a gated capture. This is
   not only cheap, it is what keeps Phase 4's rate-limiter exemption from becoming a free
   probing oracle.
3. **It is exempt from the failure budget.** `Verdict::counts_as_an_attempt()` returns false
   for `Environment`, so weather cannot disable face unlock. `Match` / `NoMatch` /
   `NotLive` / `NoFace` still count; the match is exhaustive so a new variant must be
   classified deliberately.

**0.04 is derived, not picked.** It is where `dark_ratio_max` stops working:
`mean(OFF)/mean(ON) ≈ F/(F+A)`, so that check holds only while `F < A/999`; the weakest
live amplitude measured is 48, giving `F < 0.048`. The gate sits just under the break-even
rather than on it, and a test pins the *relationship* so re-tuning either keeps them
consistent. It clears the noisiest measured dark capture (0.0020) by 20×.

**Accepted cost:** half-closed blinds (floor 0.92) are gated out. No working case is lost —
`dark_ratio_max` already rejected that capture at 0.026 against its 1e-3 — but a confusing
biometric failure becomes an honest "too bright to verify".

**Still open:** no attack recording exists between 0.92 and 20.33, so the level at which
attack/live separation actually dies is bracketed, not located. The ambient sweep closes
that, and until it does the gate is placed by the dark-check derivation rather than by
measured separation.

---

The reasoning that led there. The ambient pedestal is the one quantity that separates
cleanly and by an unbounded margin:

```text
nocturnal (7 captures, incl. blinds-closed midday)   0.00
daylight  (3 captures)                        20.33 – 51.74
```

Above an ambient level, this hardware **cannot verify liveness at all** and must say so:
fail closed, fall back to password, and report it as an *environment* outcome rather than a
failed attempt — so Phase 4's rate limiter and lockout do not punish the user for the
weather. That verdict category has to exist before Phase 3 freezes the IPC contract.

Below that level, the difference-domain checks work with their nocturnal margins intact.
Where exactly the gate sits is unmeasured: it needs the ambient sweep, because the corpus
currently holds only "dark" and "blinds open" and nothing in between.

### What survives, and what must be requalified

- **[B9](#b9)'s conjunctive rule stands.** It is the constraint that ruled out the
  otherwise-obvious burst-level fix.
- **P1-e stands, and was never at issue.** Tier is a hardware property; this hardware
  strobes 24/7. The sun did not change the tier, it broke the observation beneath it.
  Wash-out still yields a reject, never a downgrade.
- **`Tier::IrConstant` is uninstantiable** in the current code and was before this bug: on
  the non-strobe path `classify` marks every frame `On`, `build_pairs` needs an On/Off
  adjacency, so it always returns zero pairs and `min_pairs >= 2` always fails. "Downgrade
  to `ir-constant`" is not a behaviour that exists.
- **The loud floodlight case lost a layer.** The neighbour-relative phase assignment
  correctly labels a bright contaminant as the OFF frame it is, so a burst that used to be
  refused as a tier mismatch now forms and is refused by the dark-frame check instead —
  rejected by the check built for it, naming itself in the verdict, but at one layer rather
  than two. That makes `dark_ratio_max` load-bearing alone for that case, which is a second
  reason its replacement cannot be rushed.
- **Detection is unaffected.** YuNet finds the face in daylight, and finds nothing in the
  empty room, the floodlight attack, or the phone spoof, exactly as before.

### The lesson

Every statistic in this project was chosen to be a ratio, deliberately and for a good
reason: [10/Probe 3b](10-hardware-probe.md#probe-3b--repeat-run-24-hours-later) found the
OFF floor moving tenfold between runs and concluded "never threshold on absolute
brightness". That conclusion was right and the inference from it was too narrow. **A ratio
is invariant to gain and a difference is invariant to offset, and ambient light is an
offset.** Choosing between them requires knowing which nuisance parameter you are actually
exposed to — and the corpus could not answer that, because it held one lighting condition
and did not say so.

The corpus is the project's evidence, and a corpus that is uniform in a variable nobody
recorded cannot reveal that the variable matters. `record.sh` now records ambient
conditions and local time ([B19](#b19)).

**And a second lesson, from the correction rather than the bug** (added 2026-08-08):

*A ratio is invariant to gain, a difference is invariant to offset — and a **shape** is
invariant to both.* Every candidate the first pass considered was a scalar summary of
brightness, so every one of them inherited the pedestal, and enumerating six of them felt
like thoroughness while being six draws from a single family. The check that survived was
already in the codebase and was never tried, because the gate that made the daylight
problem visible also hid the one measurement that answered it.

*Check what a control group is a control for.* The matched pair was designed carefully —
same room, same position, 90 seconds apart, ambient within 1% — and that care is exactly
what made its conclusion persuasive. Both arms contained a live face. No amount of
experimental control rescues a comparison whose two arms are the same class; the
sidecar said so in plain English (`"face in frame"`) and it was read as a detail rather
than as the design of the experiment.

*When a claim rests on "their hardware is better", check that it is not your hardware.*
The optics story was plausible, well-sourced, and physically correct about 940 nm — and
irrelevant, because the comparison machine was this machine. A single sentence in the
report ("same hardware where I used Windows") was sufficient to refute a paragraph of
correct physics.

---

## B17 — The response test does not reject a screen {#b17}

**Severity: high**, and it is a *design* finding rather than a coding defect. Recorded the
first time a screen was ever put in front of the camera.

### What the design predicted

[05](05-vision-pipeline.md#the-strobe-difference-test):

> **Response test**: a live face produces a large positive `D`. A screen produces
> ~0 — LCD/OLED emit essentially nothing in NIR. Fails a phone/laptop replay outright.

### What `spoof_phone` measured

A phone at maximum brightness showing a frontal photo of the operator, ~25 cm, recorded
with a same-session `live_lit_frontal` control:

```
class       n     D min      D max
live        5    48.576     75.482
spoof       1    36.173     36.173
attack      1    37.288     37.288
null        1    55.532     55.532
```

**`D` is 36.2, not ~0.** The prediction is wrong, and the reason is that the pre-box
response test is computed over the **whole frame**, which is mostly room. The emitter lights
the wall, the desk and the operator's hands behind the phone perfectly well; the phone
contributes almost nothing and is a small part of the frame. `D` is therefore measuring the
*background*, not the presented object.

The `null_empty_room` row has been saying this all along and it was read as a curiosity
rather than a warning: an empty room scores 55.5, higher than several live captures. A
statistic that cannot distinguish a face from a wall was never going to distinguish a face
from a phone in front of a wall.

### Was the attack rejected?

**Yes — by detection, not by liveness.** End-to-end the verdict is `NoFace`: YuNet finds
nothing in a phone screen under NIR, because the screen is genuinely dark at 850 nm. The
pipeline denies it, and there is now a regression assertion that it must continue to.

But the rejection comes from a stage that was never designed to be a liveness defence, and
that matters for what happens next:

- The residual gap that *does* exist — 36.2 against a live minimum of 48.6 — is **1.34×**.
  Against the dark-frame check's 65×/130× margins that is nothing, and it is one sample of
  one phone in one room. It cannot carry an enforced threshold.
- The untested case is a screen bright enough in NIR for YuNet to find a face in it. A
  larger, brighter laptop panel is the obvious candidate. Detection would then succeed and
  the response check over the **face box** — not the whole frame — becomes the defence.
  That measurement does not exist.

### Consequences

1. `response_min` stays **advisory**. The corpus does not support enforcing it, and the
   number that would separate this one sample has 1.34× of margin.
2. **The response test needs restating in [05](05-vision-pipeline.md).** As specified it
   is a whole-frame early-out, and a whole-frame early-out cannot reject a screen. The
   version that could is the one restricted to the detected face region, which only exists
   when detection succeeds — i.e. exactly the case not yet recorded.
3. `spoof_laptop` moves from nice-to-have to **the next recording that matters**, alongside
   the print.

### Lesson

**A check that has never seen its negative class is a hypothesis, not a defence.** The
response test read as the easy one — screens are dark in NIR, everybody knows that — and it
was the falloff test that carried all the doubt. One three-second recording moved the
response test from "obviously fine" to "does not work as specified", and it cost nothing.
The checks nobody is worried about are the ones with no evidence behind them.

---

## B16 — Alignment sampled half a pixel away from ArcFace's convention {#b16}

**Severity: high.** The silent-drift bug [05](05-vision-pipeline.md#alignment) names as
"the classic bug in every reimplementation of this pipeline", found in our own
reimplementation, exactly where it said to look.

### What happened

`align()` warped the face into the canonical 112×112 chip by inverse mapping, sampling the
source at

```rust
let (sx, sy) = inverse.apply(dx as f32 + 0.5, dy as f32 + 0.5);
sample_bilinear(frame, sx - 0.5, sy - 0.5);
```

i.e. treating destination pixels as having centres at half-integer coordinates. That is
`cv2.resize`'s convention, and it is what `detect`'s letterbox correctly uses.

**`cv2.warpAffine` does not use it.** It maps destination pixel `(dx, dy)` straight through
the inverse matrix with no offset. And `warpAffine` is what produces canonical ArcFace
chips — insightface's `norm_crop` calls it — so it is the convention every embedder of this
family was *trained* against.

At an inverse scale of ~1.42 the mismatch is a systematic shift of

```
0.5 * 1.42 - 0.5  =  ~0.21 px
```

applied to every sampled point of every chip.

### Why nothing else caught it

Everything self-consistent passed. The transform estimator agreed with OpenCV's to 1e-4.
The sum of squared residuals matched to 3e-5. Alignment was byte-deterministic, the inverse
round-tripped, a known transform was recovered exactly, and the identity was the identity.
Cross-capture matching worked — 4/4 live captures matched, worst genuine similarity 0.60.

**A half-pixel offset applied uniformly is invisible to every test that compares our output
to our own output.** Enrolment and verification both shifted by the same 0.21 px, so they
agreed with each other perfectly. The only thing it degraded was agreement with the space
SFace was trained in, and no internal test can see that.

The golden comparison saw it immediately:

```
                        mean |diff|   max   within 2 levels
before (half-pixel)           3.196    87              7.6%
after  (OpenCV convention)    0.000     1            100.0%
```

Note the first attempt at this comparison used a `(x*7 + y*13) % 256` source pattern, which
wraps; a sub-pixel difference beside a wrap boundary swings ~250 levels, giving mean 7.46
and masking the signal in noise. Switching to a 16×24 checkerboard — mostly flat interiors,
high contrast only at block edges — made the residual legible. **The test data has to be
chosen so the defect is louder than the tolerance.**

### Consequences

- Fixed: `align` samples at integer destination coordinates.
- The two conventions now **differ between modules on purpose** — `detect`'s letterbox
  offsets by half a pixel because `cv2.resize` does, `align` does not because
  `cv2.warpAffine` does not. Matching each OpenCV function individually is what matters;
  being internally uniform would be wrong.
- Measured effect on matching was small (worst genuine similarity 0.6003 → 0.5921, within
  noise) — but "small on four captures of one subject" is not evidence it is small in
  general, and it was free to fix.

### Lesson

**A golden generated from your own output only catches drift. It cannot catch being wrong
from the first commit, and it will happily freeze the wrong answer forever.** The reference
has to come from somewhere else. This one cost one `apt install`, one throwaway venv and
about forty lines of Python, and it found a defect that four independent self-consistent
tests could not.

Corollary, from the tolerance: the assertion that eventually caught this was nearly written
loose enough to pass. A mean difference of 3.2 was assumed to be interpolation rounding
because interpolation rounding was the expected answer. **A tolerance wide enough to
swallow a real defect is not a tolerance** — when a residual is larger than it should be,
find out why before widening the bound.

---

## B15 — A benchmark that measured the boost clock {#b15}

**Severity: medium.** Would have put a comfortable, wrong number into
[06](06-performance-budget.md) and hidden that the budget has no headroom.

Phase 1's per-stage latency test used 12 iterations with no warm-up and reported **SFace
at 39 ms**, against the 85 ms already recorded from the Phase 0 spike. A 2.2× discrepancy
in the largest term of the budget, in the flattering direction.

Re-running the spike binary on the same machine in the same session:

```
SFace 112x112   load 199.0 ms   p50 76.19   p95 76.68   min/max 38.98 / 76.92
```

**Bimodal.** The minimum is 38.98 ms and the median is 76.19 ms, because the CPU boosts
for the first handful of inferences and then settles. A 12-sample median lands entirely
inside the boost window; the spike's 50 samples after 5 warm-up do not.

Fixed by 10 warm-up plus 50 measured iterations, which reproduces 76.9 ms and agrees with
the spike. The real correction to [06](06-performance-budget.md) was therefore
85 → 76.9 ms, not 85 → 39.

### Lesson

**A benchmark that does not outlast the boost clock is measuring the clock, not the code.**
Warm-up is not a formality and the sample count has to be large enough that the steady
state dominates the median. A `min` far below the `p50` is the tell, which is the argument
for reporting a distribution rather than a single number — the spike already did, and that
is what caught this.

Same family as [B11](#b11): a measurement was believed because it was convenient. The
control in both cases was running the other implementation on the same box.

---

## B14 — Detection threshold rejected a live face {#b14}

**Severity: medium.** A false reject on an ordinary head turn.

`DetectConfig::score_min` was set to 0.6 by analogy with OpenCV, whose own YuNet default is
0.9. Measured on the corpus at 192×192:

```
live_lit_frontal          0.905      live_lit_glasses          0.890
live_dark_frontal         0.905      live_lit_angle_left       0.549   <- rejected at 0.6
attack_floodlight_remote  0.078                                        <- noise, 553x552 box
null_empty_room           (none)
```

An angled IR face scores 0.549 — below the invented threshold and nowhere near OpenCV's
0.9. Set to **0.3**, which sits ~4× above the noise and ~1.8× below the weakest real face.

Detection is not a security boundary — a spurious box still has to clear liveness and
matching — so it is tuned for recall. One subject, four captures: provisional.

**Lesson:** a threshold from another project's README is calibrated for that project's
data. Ours is greyscale NIR at 640×360; theirs is not.

---

## B13 — Detector input size chosen without ever measuring accuracy {#b13}

**Severity: high.** Silently degraded every embedding, which is the exact failure mode
[05](05-vision-pipeline.md#alignment) warns about — no error, no log line, just a worse
system.

### What happened

`DetectConfig::input_size` was set to 192×192 because [06](06-performance-budget.md) had a
row saying YuNet costs 11 ms there. That is a latency argument, and input size is not only
a latency parameter: a 640×360 frame letterboxed to 192 puts a laptop-distance face at
~45 px, and its five landmarks then scale back up 3.3× into the alignment.

Landmark displacement against a 448×448 reference, full-frame px:

```
size   mean-to-max displacement   YuNet
 160          6.3 - 10.9 px        8.2 ms
 192          5.1 - 12.1 px       11.5 ms
 320          1.8 -  6.2 px       33.0 ms
```

A displacement is not yet a cost, so it was converted into one — embed the same frame from
each size's landmarks and take the cosine against the reference:

```
size   worst embedding cosine vs 448
 160                        0.8861
 192                        0.8235
 320                        0.9368
```

Read against a perturbation curve measured on the same frames — 1 px costs 0.993, 4 px
0.960, 8 px 0.911 — **running at 192 costs more than 8 px of landmark error.** Genuine
cross-capture similarity for the same person reads 0.60–0.90, so detector resolution alone
was eating a large share of the genuine/impostor margin.

Note also that 160 and 192 are **not ordered with respect to each other**. At that scale
the landmark regression is noisy rather than converging, which is a second reason not to
sit there.

### Consequences

- `input_size` default is **320**. Costs 22 ms.
- Detection reverts to **once per burst**, its landmarks shared across the embedded
  frames — which is what [06](06-performance-budget.md) always budgeted. Phase 1 had
  briefly detected per frame to avoid a correlated failure; the measurement says sharing
  landmarks across frames ~66 ms apart costs whatever the face moved in 66 ms, which at
  2 px is 0.98 and at 4 px is 0.96 — an order of magnitude cheaper than the resolution.
  k-of-n keeps its independence regardless, because three different *images* are still
  embedded and only the alignment is shared.

### Lesson

**A parameter that appears in the latency table is not therefore a latency parameter.**
The budget made input size look like a pure speed knob because that is the only column the
budget has. The question "what does this cost in accuracy" was never asked, and it had a
cheap answer available the whole time: perturb the input, measure the output.

---

## B12 — Detection ran on emitter-OFF frames {#b12}

**Severity: medium** as a defect, high as a repeat.

The first run of Phase 1's detection test found **zero faces in every capture**, which
reads like "YuNet does not work on IR" — a conclusion that would have invalidated the
model choice.

It was reading frame index 6. On the reference machine the frame sequence past AGC settle
is `0.001, 59.91, 0.004, 61.92, …` — index 6 is an emitter-**OFF** frame at 0.004 mean
luma. **It was detecting on black.**

Fixed by selecting the brightest frame in the window rather than an index. Every live
capture then detected at 0.549–0.905.

### Lesson

This is [B5](#b5) arriving from a different direction. B5's rule was written for capture:

> ON/OFF phase must be re-established by measuring luma on every capture, never by
> assuming frame `n` is ON because frame `n-1` was OFF.

The rule was obeyed in `frame.rs`, which classifies by luma, and then broken immediately
in the test harness that fed it. **A rule that lives in one module is not a rule.** Any
code that picks a frame — production, test, or throwaway — picks it by measurement.

Also worth recording: the symptom pointed at the wrong layer entirely. "No faces found"
looked like a model or decode problem and was an input-selection problem, and the thing
that distinguished them was that the *empty room* also found nothing, i.e. the result was
identical for a frame with a face and a frame without one.

---

## B11 — Cross-machine benchmark comparison {#b11}

**Severity: medium.** Wrong conclusion about the central Q2 trade-off, stated confidently,
from a measurement that could not support it.

### What happened

With only the `tract` half of the S1 spike run, the numbers were compared against the
"5.6 ms" YuNet figure in [06](06-performance-budget.md) — which comes from OpenCV Zoo's
published benchmark. Normalising per pixel gave:

```
OpenCV published   5.6 ms @ 160x120  =  0.29 us/px
tract measured     7.7 ms @ 160x160  =  0.30 us/px
```

and the conclusion recorded was *"tract is within ~5% of the C++ reference; the premise
that pure Rust costs a large constant factor does not hold."*

**The two numbers are from different computers.** OpenCV's benchmark hardware is not
this laptop, and its published figures are not dated to a CPU we have. Per-pixel
normalisation controls for input size; it does nothing about the machine. The comparison
was meaningless.

### What the same-machine measurement showed

Once `ort` ran on the same box with the same input buffer:

```
YuNet 160x160    tract 7.69 ms    ort 0.45 ms    17x
YuNet 192x192    tract 11.15 ms   ort 1.27 ms    8.8x
YuNet 320x320    tract 33.94 ms   ort 1.67 ms    20x
SFace 112x112    tract 85.25 ms   ort 30.85 ms   2.8x
```

Not 5%. **Between 3× and 20×.** The original "pure Rust is slower" premise in Q2 was
correct, and the analysis had talked itself out of it.

### Why it mattered

Q2 is the decision this spike exists to inform, and the erroneous finding pointed at
the answer the project was already leaning toward. A measurement that conveniently
confirms the existing preference is the one that most needs a control, and it did not
get one until the other half of the spike ran.

The decision does not actually flip — `tract` still clears the latency budget at
~318 ms and still wins on attack surface — but it was very nearly made on a number that
was not real.

### Lesson

**A benchmark is only a comparison when both sides run on the same machine.** Published
figures set expectations; they do not settle A-versus-B. If a spike exists to compare
two things, it is not finished until both have been measured under it — reporting half
of a comparison as though it were the whole is how this happened.

Related, from the same pass: "non-square input fails" was recorded as a `tract` defect
before `ort` was tested. `ort` fails identically — it is a limitation of the YuNet model.
Same root cause: one implementation was measured and its behaviour attributed to the
implementation rather than to what was shared.

---

## B10 — EdgeFace weights are non-commercial, not BSD-3 {#b10}

**Severity: critical.** Kills [Q1](08-open-questions.md#q1--embedding-model-sface-or-edgeface--decided-edgeface)'s
decision outright, and would have been a licence violation shipped in a Debian package.

### What happened

[08/Q1](08-open-questions.md) staged exactly this check:

> - [ ] Confirm EdgeFace's BSD-3 LICENSE covers the **trained weights**, not just code

The GitHub repo makes it look fine. `LICENSE` at the root is a standard, unmodified
BSD 3-Clause with no non-commercial carve-out, and the weights are committed *inside
that repo* at `checkpoints/edgeface_xs_gamma_06.pt`. `checkpoints/README.md` mentions no
separate terms. On that evidence the answer is "yes, BSD-3 covers the weights".

**That evidence is incomplete and the conclusion is wrong.** The authors also publish
the same weights on HuggingFace, and there they declare a different licence:

```
Idiap/EdgeFace-XS-GAMMA -> cc-by-nc-sa-4.0
Idiap/EdgeFace-Base     -> cc-by-nc-sa-4.0
Idiap/EdgeFace-S-GAMMA  -> cc-by-nc-sa-4.0
Idiap/EdgeFace-XXS      -> cc-by-nc-sa-4.0
```

Not an inference from a repo tag — it is in the model card's own frontmatter:

```yaml
---
license: cc-by-nc-sa-4.0
---
```

**CC BY-NC-SA 4.0 is NonCommercial.** That fails DFSG §6, "No Discrimination Against
Fields of Endeavor". Debian cannot ship it. Neither can we, given
[Q6](08-open-questions.md#q6--target-debian-archive-or-ppa-and-see--decided-archive)
is decided and binding.

### Why the two licences disagree

The model card states EdgeFace was **trained on WebFace260M**, whose own terms are
research-only. A permissive licence on training code and a restrictive one on the
resulting weights is the consistent, deliberate position — not a mistake. It is exactly
the InsightFace situation already ruled out in [05](05-vision-pipeline.md#embedding):
*code MIT, weights non-commercial.*

The project walked into the same trap a second time, with a different model.

### Consequences

- **Q1 reverses to SFace** (Apache 2.0, OpenCV Zoo). The embedder trait staged in
  [05](05-vision-pipeline.md#embedding) is what makes this a config change instead of a
  rewrite — that abstraction has now paid for itself.
- The latency budget gets worse: SFace measured **85 ms** under `tract` against
  EdgeFace's projected ~10–15 ms. Still inside the p95 target, but the headroom is gone.
  See [B11 note in Q2](08-open-questions.md#q2--inference-runtime-ort-tract-or-candle).
- 512-d embeddings become 128-d, and ~7 MB becomes ~37 MB.

### The lesson, which is the reusable part

**A licence file in a source repo does not license the weights inside it, and finding a
permissive LICENSE is not the end of the check — it is the beginning.** Weights are a
separate artifact with separate provenance, and the authoritative statement is wherever
the authors *distribute the weights*, which is frequently not the code repo.

The check that actually works, and is now mandatory before any model enters `models/`:

1. Licence at the **weight distribution point** (HuggingFace card, release asset,
   model zoo per-model `LICENSE`), not just the source repo root.
2. **Training-dataset terms.** WebFace260M, MS-Celeb-1M, and CASIA-WebFace are all
   research-only, and a model trained on them usually inherits a restriction.
3. Whether the redistributor had the right to grant what they granted.

SFace passes 1 (OpenCV Zoo ships a per-model Apache 2.0 `LICENSE` beside the ONNX) and
3 is credible. Point 2 carries a residual, documented risk: the upstream
[zhongyy/SFace](https://github.com/zhongyy/SFace) repo declares no licence at all, and
the training data is not stated in the zoo's model card. **This is not yet fully
cleared** and must be before an ITP.

dlib's `dlib_face_recognition_resnet_model_v1` remains the fallback-of-last-resort at
**CC0 / public domain** — unambiguous, and the reason Howdy has no licence problem. It
costs accuracy (99.38% LFW) and is a 2017 ResNet-29, but it is the only candidate whose
licensing is beyond argument.

---

## B9 — Dark-frame integrity check is defeated by a pulsed IR source {#b9}

**Severity: critical.** This is an authentication bypass.

### What happened

`attack_floodlight_remote` was recorded by holding an ordinary TV remote against the
lens — the external-IR-floodlight attack from
[05](05-vision-pipeline.md#the-strobe-difference-test). The check is meant to notice
that the OFF frame is no longer dark and reject.

Per-pair, it mostly does. But not always:

```
pair   mean(ON)  mean(OFF)      OFF/ON
   0    60.0472    0.00026     4.36e-6   <-- reads as perfectly clean
   1    82.9296    9.29104     1.12e-1   <-- caught
   2    88.8702    0.00016     1.77e-6   <-- reads as perfectly clean
   3    91.1450    8.56571     9.40e-2   <-- caught
   4    92.6711    0.00026     2.83e-6   <-- reads as perfectly clean
```

**7 of 42 pairs (16.7%) read as completely clean.** An IR remote does not emit
continuously — it sends a modulated packet and then idles before repeating. Any OFF
frame landing in a gap is a genuine dark frame, and the check has nothing to complain
about.

### Why it is critical

[05](05-vision-pipeline.md) budgets **4 frames = 2 ON/OFF pairs** per authentication.
At a 16.7% per-pair pass rate, an attacker whose source happens to be pulsed gets both
pairs clean **2.8% of the time — roughly 1 attempt in 36.**

For a factor that authenticates `sudo` and the lock screen, 1-in-36 is not a
mitigation, it is a door. And a deliberate attacker would pulse *on purpose*, because
they do not need to know our frame timing — they only need to be dark often enough to
get lucky, and they can retry.

### The fix that did not work

The obvious idea was the motion-consistency check already in the design: "the OFF
frames should be near-identical over ~100 ms". A pulsed source makes them wildly
inconsistent, so the coefficient of variation of `mean(OFF)` across the burst should
expose it. Measured:

```
attack_floodlight_remote   mean(OFF) avg=5.65887  CV=0.54
live_lit_frontal           mean(OFF) avg=0.00016  CV=0.42
live_lit_frontal           mean(OFF) avg=0.00014  CV=0.65
null_empty_room            mean(OFF) avg=0.00085  CV=0.33
```

**CV does not separate them at all** — 0.54 for the attack sits inside the live range
of 0.33–0.65. A live OFF frame is pure sensor read noise, and read noise is *relatively*
noisy even though it is absolutely tiny. Relative spread is the wrong statistic here.

### The fix that works

Aggregate the per-pair ratio **conjunctively**: reject if *any* pair in the burst shows
contamination, rather than thresholding a median or requiring k-of-n to pass.

```
RECORDING                          median          MAX   verdict@1e-3
attack_floodlight_remote         7.18e-02     1.30e-01         REJECT
live_dark_frontal                3.21e-06     5.36e-06          allow
live_lit_angle_left              1.88e-06     5.03e-06          allow
live_lit_frontal                 2.00e-06     3.76e-06          allow
live_lit_frontal                 1.45e-06     4.43e-06          allow
live_lit_glasses                 1.73e-06     3.88e-06          allow
null_empty_room                  1.00e-05     1.54e-05          allow
```

Worst live reading `1.54e-05`, attack `1.30e-01` — **four orders of magnitude apart**.
A threshold at `1e-3` sits ~65× above the worst honest capture and ~130× below the
attack. Zero false rejections across **210 live pairs**.

### Consequences, now binding

1. **Liveness aggregation is conjunctive; matching aggregation is k-of-n.** These are
   opposite rules and must not be confused. `05` already specifies "require k
   consecutive agreeing frames" — that is about *identity*, where the risk is a
   single-frame fluke. For *liveness* the risk runs the other way: one contaminated
   frame is proof of an attack, and the burst must fail as a whole. Any refactor that
   unifies them reintroduces this bug.
2. **A liveness check is a property of the burst, not of a frame.** Per-frame verdicts
   must never be reduced by voting.
3. The 2-pair capture budget in [06](06-performance-budget.md) was sized for latency,
   not for attack margin. Conjunctive aggregation is what makes 2 pairs safe; without
   it, the margin needed ~6 pairs and would have blown the latency budget.

### What is still unknown

The remote is a *pulsed* source with gaps we did not choose. A continuous IR
illuminator would be caught on every pair and is the easier case. An attacker who
synchronises their pulse to the sensor's strobe phase is a harder case and is **not**
tested by this corpus — it needs a controllable IR source. Queued in
[10](10-hardware-probe.md#still-to-probe).

---

## B8 — Liveness cannot run before detection, but the pipeline says it does {#b8}

**Severity: high.** Internal contradiction in the design, surfaced by trying to build it.

[05](05-vision-pipeline.md) opens with:

```
V4L2 capture ─→ IR liveness ─→ detect ─→ align ─→ embed ─→ match
```

> Liveness runs **first**, before recognition.

But the strobe-difference test in the same document says:

> `D = mean(ON) - mean(OFF)`, restricted to **the detected face region**.

Both cannot be true. The falloff test needs a face box, and the face box comes from
YuNet, which the diagram places *after* liveness.

**Resolution:** detection runs first, on the ON frame. Liveness then uses that box.
The principle being protected — *never embed or match something that has not passed
liveness* — is intact, because embedding and matching still come last. It is only
detection that has to move, and detection is 85 KB and ~5.6 ms.

Corrected pipeline:

```
V4L2 capture ─→ detect ─→ IR liveness ─→ align ─→ embed ─→ match
                (YuNet)   (uses the box)
```

The cheap checks that need no box — dark-frame integrity, whole-frame response — can
still run before detection as an early-out, and should, since they reject a screen
without paying for detection at all.

---

## B7 — Centre-crop ROI does not cover the face {#b7}

**Severity: high** — it is what blocks the project's one unproven claim.

`spikes/probe` used a centred 50% crop as a stand-in for a face box, on the assumption
that at laptop distance the face fills the middle of the frame. The cell maps say
otherwise. Mean response per cell, % of peak:

```
live_lit_frontal                     null_empty_room
   8   16   14   12   48  100          70   76   76   72   66   53
   9   10   10   66   47   94          77   82   81   78   72   59
   9    5   21   79   52   76          84   88   84   81   78   65
   6   25   38   74   69   77          90   95   90   87   84   70
  19   44   53   87   80   91          94  100   99   96   90   75
  39   54   57   92   79   94          77   66   73   80   81   71
```

The "frontal" capture peaks at the **bottom-right corner** and is nearly dark at the
top-left. The subject is at the edge of the ROI, not in the middle of it.

Consequence: `falloffCV` is measuring a subject-versus-background gradient, not facial
curvature. Two recordings of the *same* pose gave **0.31 and 0.64** — a 2× spread that
is entirely explained by the operator sitting in a slightly different place.

**This blocks S5 independently of the missing print sample.** Even with a print in hand,
comparing its falloff to a live face is meaningless while neither measurement is taken
over the face. Detection has to land first — which is the same conclusion as
[B8](#b8), reached from the other direction.

Marked in the source as a known ceiling:

```rust
// ponytail: centre crop instead of a real face box. [...] Swap for the YuNet
// detection box in Phase 1 -- at that point the ROI tracks the face instead of
// assuming it is centred, and off-centre captures stop being noise.
```

The metric itself is sound — `domed_response_has_higher_falloff_cv_than_flat` passes on
synthetic data, so the arithmetic distinguishes a curved reflector from a flat one. It
is the input that is wrong, not the formula.

### CLOSED, Phase 1 (2026-08-04)

YuNet landed and the ROI is now the detected face box. Falloff CV over the corpus, worst
pair per capture:

```
live_lit_frontal     0.3130      live_lit_glasses     0.2989
live_dark_frontal    0.2900      live_lit_angle_left  0.2661
```

**0.266–0.313, a 1.18× spread**, against the centre crop's 0.31 and 0.64 for the *same
pose*.
The prediction that the old spread was operator position rather than facial curvature
holds: with the ROI tracking the face, it mostly goes away.

This is what set the degenerate-flatness floor at 0.05 —
[05](05-vision-pipeline.md#liveness--the-differentiating-piece), `config.rs`. Replayed in
CI by `tests/liveness_corpus.rs`.

**What this does *not* close:** the live-versus-print separation. B7 was one of two
blockers on that claim and it is the one that is gone. The other — no printed photo has
ever been recorded — remains, and until it is, the falloff check claims only that it
rejects a perfectly flat presentation.

---

## B6 — `by-id` symlink resolves to the wrong camera {#b6}

**Severity: high.** Would have authenticated off the RGB sensor while believing it held IR.

[04](04-architecture.md) and [05](05-vision-pipeline.md) both specified persisting the
camera as a `/dev/v4l/by-id/` symlink, because `videoN` renumbers across boots. Correct
problem, wrong fix.

Both cameras are interfaces of one USB device reporting a single serial (`SN0001`), so
udev generates a colliding basename and the last writer wins. Four nodes, two symlinks,
**neither pointing at the IR camera**:

```
usb-Sonix_..._SN0001-video-index0 -> ../../video0   <-- RGB camera
usb-Sonix_..._SN0001-video-index1 -> ../../video3   <-- IR *metadata* node
```

Fixed by switching to `by-path`, where the USB interface number separates them
(`:1.0` RGB, `:1.2` IR). Full write-up:
[10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation).

### Recurrence, Phase 1

The same class bit again, in the golden-test harness. Detection goldens were keyed by the
capture's **label**, and labels are not unique — a second `live_lit_frontal` take was
recorded as a same-session control and the lookup silently returned the *first* take's
golden. Our decode was compared against OpenCV's reading of a different recording, from a
different day, with the operator sitting somewhere else. It surfaced as `box IoU 0.699`,
which reads like a decode defect and was nothing of the kind.

Fixed by keying on the filename stem, which carries the UTC timestamp. **The lesson
transferred but the vigilance did not**: the wiki already said a stable-looking name that
resolves to the wrong object is the dangerous kind, and the harness was written with a
non-unique key anyway. Worth noting that this one failed *loudly* — an assertion caught
it — which is the difference between B6 and this, and the whole argument for the
assertions being tight.

**Lesson:** a stable name that silently resolves to the wrong device is worse than an
unstable name that fails loudly. The daemon now re-verifies format and geometry on open
regardless of what the config says, because the config can be stale or wrong and one
ioctl is cheap against authenticating off the wrong sensor.

---

## B5 — ffmpeg silently duplicates frames when writing to disk {#b5}

**Severity: high.** Would have silently corrupted the entire corpus.

`-f rawvideo` rate-converts output to constant 30 fps and duplicates frames to fill it.
A 30-frame capture came back as 15 distinct sensor frames, each written twice, turning
the sensor's `ON,off,ON,off` into `ON,ON,off,off`.

The file is the right size. Every frame is valid. Nothing errors. Only the pairing is
wrong — and the pairing is what every liveness check depends on.

Fixed with `-fps_mode passthrough`. Full write-up:
[10/Probe 3c](10-hardware-probe.md#probe-3c--recording-to-disk-duplicates-frames-unless-you-stop-it).

**How it was caught:** the probe's output was cross-checked against the known-good
`ffmpeg signalstats` numbers already in [10](10-hardware-probe.md), and every luma value
appeared exactly twice. Without a prior independent measurement to compare against,
this would have looked like plausible data.

**Lesson, and it generalises past ffmpeg:** never assume the frames you were handed are
the frames the sensor produced. `faciolockd` reads V4L2 buffers directly and avoids this
specific bug, but dropped, repeated, and reordered buffers under load are the same
class. **ON/OFF phase must be re-established by measuring luma on every capture**, never
by assuming frame `n` is ON because frame `n-1` was OFF.

---

## B4 — Undocumented capture joins the corpus {#b4}

`live_lit_frontal_20260803T183008Z.gray` exists as a complete 90-frame file with no
`.json` sidecar, so there is no record of what was in front of the camera. It cannot be
trusted as a positive or a negative sample.

The sidecar is deliberately written *after* ffmpeg succeeds, so a sidecar always
describes a real file — but the converse was not enforced. `spikes/probe` now warns on
a missing sidecar rather than analysing it silently.

Root cause not established: the capture is byte-exact at 90 frames, so ffmpeg completed
and the script exited between that and the sidecar write. Interrupted run is the likely
explanation. Flagged rather than guessed at.

---

## B3 — `2>/dev/null` hid a missing binary as "no evidence" {#b3}

Probe 1 originally printed four device names and nothing else. `v4l2-ctl` was not
installed, and the redirect swallowed "command not found" — leaving output that read
like "there is nothing there" rather than "the tool is missing".

**Lesson:** a probe that degrades silently to "no evidence" is worse than one that fails
loudly, because absence of evidence reads as evidence of absence. Device enumeration in
the daemon must distinguish *absent*, *inaccessible*, and *present-but-not-IR* rather
than collapsing all three into an empty result.

---

## B2 — Half the `/dev/videoN` nodes are not cameras {#b2}

`/dev/video1` and `/dev/video3` are UVC payload-header metadata nodes (`UVCH`), one per
camera. They expose no capture formats and can never yield a frame.

Enumeration must filter on `V4L2_CAP_VIDEO_CAPTURE` before applying any IR heuristic.
Without it, the "capture one frame and check chroma variance" fallback in
[05](05-vision-pipeline.md#finding-the-ir-camera) is aimed at nodes that will never
answer. Details: [10/Probe 4](10-hardware-probe.md#finding-2--half-the-nodes-are-not-cameras).

---

## B1 — Dead conditional in `build_pairs` {#b1}

A leftover `if` with an empty body and a comment claiming the work happened elsewhere.
Removed before it ever compiled. Noted only because a conditional that looks like it
enforces something and enforces nothing is exactly the shape of a bug that survives
review.

---

## Environment friction (not bugs, but they cost time)

| Thing | Resolution |
|---|---|
| `sudo` needs a TTY the agent does not have | user runs privileged commands |
| `cargo` from repo root fails — no workspace yet | `--manifest-path spikes/probe/Cargo.toml` |
| `cargo clippy` not on `PATH` | lives at `/usr/lib/rust-1.93/bin/cargo-clippy` |
| `cargo fix` refuses without VCS | applied the two lints by hand |
| `mawk` has no `asort` | hand-rolled the sort |
