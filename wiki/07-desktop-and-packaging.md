# 07 — Desktop Integration & Packaging

## Integration surface

| Surface | PAM service | Notes |
|---|---|---|
| `sudo` | `sudo` | easiest; best first target |
| `su` | `su` | trivial once sudo works |
| KDE lock screen | `kde` | kscreenlocker's own service name |
| SDDM greeter | `sddm` | display-manager login |
| polkit prompts | `polkit-1` | KDE auth dialogs |
| TTY login | `login` | camera may not be initialised this early |
| GDM/GNOME (untested) | `gdm-password` | for portability beyond Kubuntu — see [Desktop compatibility](#desktop-compatibility) |

## KDE specifics — the important one

kscreenlocker requires PAM for unlocking (service `kde` by default). Two facts drive
our design:

1. **KDE runs multiple PAM stacks concurrently.** This was added specifically so
   password and fingerprint can be offered at the same time. The stack that finishes
   first wins; the other is cancelled.
2. **The documented pain point** from that work: a biometric method that can cancel
   at any time, combined with a blocking password prompt, causes the UI to stall —
   the fingerprint succeeds but nothing happens until you press Enter. Their fix was
   the dual-stack approach.

Consequences for us:

- **We must handle cancellation cleanly.** When KDE cancels our stack because the
  password won, we must close the socket immediately and not leave the camera open.
  The daemon must treat socket close as "abort now", release the device, and log the
  attempt as cancelled rather than failed (so it doesn't feed the rate limiter).
- **We must never need `/dev/uinput`.** Howdy's Enter-key injection ([02/W3](02-howdy-teardown.md#w3--the-devuinput-enter-key-injection))
  exists to work around exactly the stall described above. With KDE's dual-stack
  model that workaround is obsolete, and it was always a bad idea from a root-
  privileged auth module.
- Warning from KDE's own docs: **if the `kde` PAM service is misconfigured you cannot
  unlock the screen.** Our installer must therefore never edit `/etc/pam.d/kde`
  directly — see below.

### And none of it matters yet, because the greeter is not root (2026-08-31)

Everything above is about what happens *after* `pam_faciolock.so` is reached. On this machine
it is never reached at all.

`kscreenlocker_greet` is `0755 root:root` — **not setuid** — links `libpam.so.0` directly, and
is started by `kwin` inside the user's session. Plasma 6.6.5 ships **no `kcheckpass`**, no
system D-Bus service and no polkit action for the screen locker; the password prompt works
because `/usr/sbin/unix_chkpwd` is setgid `shadow`. So the whole `kde` stack, `common-auth`
included, runs as uid 1000 — and `/run/faciolock.sock` is `0600 root:root` with a
`SO_PEERCRED` root check behind it. `connect()` is `EACCES`, the module returns
`PAM_AUTHINFO_UNAVAIL`, and the lock screen silently falls through to the password field
forever.

This is [P6-a](README.md#decisions-taken)'s wall reached from the other direction: a user
session cannot talk to a root socket. `sudo`, `su`, `login` and `sddm` authenticate as root
and are unaffected.

**Resolved the same day, [P6-d](README.md#decisions-taken):** the socket is now `0660
root:faciolock-auth` and the rule moved from the file mode onto `SO_PEERCRED` — root asks
about anybody, anybody else asks about themselves and only `Auth`/`Status`. So the lock screen
works **once you are in the group**, which is a deliberate second step and not part of the
install:

```bash
sudo gpasswd -a $USER faciolock-auth
```

Group membership is read at session start, so it takes a log-out and back in. Until then the
lock screen behaves exactly as it did before — `EACCES`, password field, nothing broken.

### When the face check runs is the greeter's decision, not ours (2026-08-31)

The first face login through a display manager happened on the log-out-and-back-in above, and
it did not look the way anyone expects:

```text
18:42:09  sddm-helper: pam_unix(sddm-greeter:session): session opened for sddm
18:42:19  faciolockd: auth uid=1000 verdict=Allow similarity=0.840 pid 17672
18:42:19  sddm-helper[17672]: pam_unix(sddm:session): session opened for user csr
```

Ten seconds between the login screen appearing and the camera being consulted — **and the
camera was consulted only because the password had just been submitted.** Same pid on the
face check and the session, so both are one `pam_authenticate()` call.

**This is not our stack order.** `pam_faciolock.so` is the Primary entry in `common-auth`, above
`pam_unix`, and it does run first *within* the stack. What is late is the stack itself: PAM is
a pull API, and SDDM's greeter collects the password field before handing it to `sddm-helper`,
which is the thing that calls `pam_authenticate()`. No module can ask to be started earlier;
there is no PAM entry point that means "begin now, nobody has typed anything".

`sudo` is the contrast and it is the same module: `sudo` calls `pam_authenticate()`
immediately, and the password prompt a user thinks of as "sudo asking" is actually
`pam_unix`'s *conversation*, further down the stack. Face reaches the daemon before
`pam_unix` ever speaks, which is why the 2026-08-31 trial returned with no prompt at all.

**The consequence worth stating plainly: when the face wins at SDDM, the password that was
typed is never checked.** `[success=2]` jumps over `pam_unix`, so its `try_first_pass` never
runs. Any string in that field, followed by a face that matches, is a successful login. That
is the intended meaning of a sufficient factor rather than a defect — but it is what the
sequence above actually did, and nobody should discover it by accident.

This is also the honest ceiling on what a face-unlock module can promise on a desktop: **the
module decides the verdict, the greeter decides the moment.** Howdy sits behind the same wall.

### The lock screen does not wait to be typed at, and that is the whole difference (2026-08-31)

Measured the same evening, and it is the opposite of SDDM. A trackpad touch woke the lock
screen and it unlocked with **no keystroke at all**:

```text
19:20:31  Started faciolock.service                    <- the wake activated it
19:20:31  kscreenlocker_greet[21264]: kde-fingerprint:auth …
19:20:33  faciolockd: auth uid=1000 verdict=Allow similarity=0.873 pid 21264 uid 1000
19:20:33  kscreenlocker_greet[21264]: kde:auth … setcred                <- unlocked
```

KDE starts its stacks when the greeter *wakes*, not when a field is submitted — the multi-stack
design described at the top of this page, running `kde`, `kde-fingerprint` and `kde-smartcard`
concurrently so a biometric can win before anyone types. `common-auth` is inside the `kde`
stack, so we are simply one of the racers. The UI shows a small **Unlock** button rather than a
bare password field while a non-password stack is live.

Two details worth keeping:

- **We run once per unlock, not once per stack.** Only `/usr/lib/pam.d/kde` `@include`s
  `common-auth`; `kde-fingerprint` calls `pam_fprintd.so` directly and `kde-smartcard` its own
  module. Had all three included it, one trackpad touch would have queued three camera bursts
  at a daemon that serves them one at a time.
- **`pid 21264 uid 1000`** in the daemon's audit line is the entire point of
  [P6-d](README.md#decisions-taken): an unprivileged session process asking about itself and
  being answered. Two days earlier the same call was `EACCES`.

So the moment is the greeter's, and the two greeters on this machine chose differently:
**SDDM asks PAM when you submit, KDE's lock screen asks on wake.** Neither is configurable
from our side, and the difference is worth stating in the README before a user concludes the
module is broken at one of them.

## Never hand-edit `/etc/pam.d/*`

Debian/Ubuntu have a framework for this. Packages ship a profile in
`/usr/share/pam-configs/` and call `pam-auth-update --package` from the maintainer
script. The profile declares the modules, options, ordering priority, and whether it's
enabled by default. `libpam-runtime (>= 1.0.1-6)` is the dependency that guarantees
`pam-auth-update` exists.

Sketch of `/usr/share/pam-configs/faciolock`:

```
Name: Face authentication (Faciolock)
Default: no
Priority: 512
Auth-Type: Primary
Auth:
	[success=end default=ignore]	pam_faciolock.so
Auth-Initial:
	[success=end default=ignore]	pam_faciolock.so
```

- `Default: no` — **opt-in**. A face-auth package must never enable itself on
  install. The user enrolls, then enables.
- `Priority: 512` puts it above `pam_unix` (priority 256) so face is tried first.
- `default=ignore` is the fail-open-to-password behaviour: any non-success falls
  through to the password stack.

This also gives us a supported uninstall path, which hand-edited `pam.d` files do not.

**The sketch above was missing a line, and Phase 4 needs it.** The profile as drafted has no
`Account:` section, and [P4-b](README.md#decisions-taken) put the lockout release in
`pam_sm_acct_mgmt`. Without an account line the module is never called after a successful
login, so the failure budget is never cleared: three refused faces disable face unlock until
somebody restarts the daemon. The shipped profile is
[`data/pam-configs/faciolock`](../data/pam-configs/faciolock) and it carries
`Account-Type: Additional` with an `optional` control — `optional` rather than `required`
because the module returns `PAM_IGNORE` on every path by design, and an account-stack failure
refuses a login outright instead of falling through to a password.

## Rolling it out, and getting back out

Verified on the reference machine, not assumed: all six services in the table above include
`common-auth` and `common-account`, so one `pam-auth-update` covers all six.

```text
/etc/pam.d/sudo   /etc/pam.d/su   /etc/pam.d/login   /etc/pam.d/sddm
/usr/lib/pam.d/kde   /usr/lib/pam.d/polkit-1
```

Note the last two: on Kubuntu 25.10 there is **no `/etc/pam.d/kde` and no
`/etc/pam.d/polkit-1`.** They are vendor files under `/usr/lib/pam.d/`, from
`libkscreenlocker6` and `polkitd`. Writing a fresh `/etc/pam.d/kde` shadows the vendor one
entirely, so a two-line file there does not add face unlock to the lock screen — it *replaces*
the lock screen's whole authentication stack with two lines. That is the misconfiguration
KDE's own documentation warns produces a screen you cannot unlock.

### The order, and why it is this order

`sudo` first, alone, and not through `pam-auth-update`. The profile is all-or-nothing by
construction — it edits `common-auth`, which every service includes — so there is no way to
enable it for one service. A first trial that can only be evaluated by locking your own screen
is not a trial. `sudo` fails safe in a way the lock screen does not: a `sudo` that refuses is
a shell you already have.

```text
1.  /etc/pam.d/sudo, by hand, one line          <- reversible from the shell you are in
2.  live with it                                <- the failure modes are boring by now
3.  pam-auth-update, all six                    <- supported, packaged, reversible
```

**Walked 2026-08-31, and step 2 was skipped.** Step 1 worked first time — `sudo` authenticated
by face with no password prompt — and the file was then restored from its backup rather than
lived with, so step 3 followed the same afternoon. What `pam-auth-update --enable faciolock`
generated:

```text
auth	[success=2 default=ignore]	pam_faciolock.so
auth	[success=1 default=ignore]	pam_unix.so nullok try_first_pass
auth	requisite			pam_deny.so
auth	required			pam_permit.so
```

Two things worth reading rather than trusting. The jump arithmetic is right: a face success
skips two lines to `pam_permit`, anything else falls to `pam_unix`, and `pam_unix`'s own
`success=1` lands on the same `pam_permit`. And **`pam_unix` gained `try_first_pass`**, which
pam-auth-update adds whenever a Primary module sits above it. That is harmless here for one
specific reason: `pam_faciolock.so` never runs a PAM conversation, so there is no stashed
password for `try_first_pass` to find and it prompts normally — observed. A future version
that ever prompted for anything would change `pam_unix`'s behaviour without touching
`pam_unix`.

### The two lines, and what makes them safe

Back the file up first — the undo below depends on it existing:

```bash
sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.faciolock-backup
```

```
auth	[success=end default=ignore]	pam_faciolock.so
```

**This was one line until 2026-08-31, and one line has a slow leak.** It is the same omission
[P6-b](README.md#decisions-taken) found in the shipped profile's sketch, arriving again in the
by-hand trial: the auth line alone never calls `pam_sm_acct_mgmt`, which is where
[P4-b](README.md#decisions-taken) put the lockout release. The failure budget is on disk and
survives every idle-exit, so three refused faces during the trial — a dark room, a bad angle,
the camera busy — disable face unlock for the rest of it, and the trial reads as "it stopped
working" rather than "it is rate-limited, as designed". So the trial carries the account line
too, above `@include common-account`:

```
account	optional	pam_faciolock.so
```

`optional` because the module returns `PAM_IGNORE` on every path by design. If the budget is
already spent, `sudo faciolockctl test $USER` clears it on the way out (P5-c) without touching
any PAM file.

`default=ignore` is the whole recovery story. **Every** outcome that is not a success —
daemon down, socket missing, not enrolled, rate-limited, refused, malformed reply, panic
caught at the FFI boundary — falls through to the next line in the file, which is
`@include common-auth`, which is the password prompt. wiki/09 Phase 3's failure table is the
evidence for that sentence: every one of those paths was observed returning
`PAM_AUTHINFO_UNAVAIL` or `PAM_USER_UNKNOWN`, and neither is a success.

The line goes **above** `@include common-auth`, and `pam_faciolock.so` must be findable —
`/usr/lib/x86_64-linux-gnu/security/` on Debian and Ubuntu.

### Getting back out

Before the edit, open a second terminal and hold a root shell in it:

```bash
sudo -i
```

That shell is the recovery path and it does not care what `/etc/pam.d/sudo` says afterwards,
because it authenticated before the edit. From it, either line undoes everything:

```bash
cp /etc/pam.d/sudo.faciolock-backup /etc/pam.d/sudo
```

```bash
sed -i '/pam_faciolock/d' /etc/pam.d/sudo
```

If both terminals are lost as well — a PAM file broken badly enough to refuse `sudo` outright
— the recovery is a root shell from the boot menu: hold Shift for GRUB, pick the recovery
entry, and `/etc/pam.d/` is a normal file on a normal filesystem. Nothing in this design can
make a machine unbootable; the worst it can do is make one service refuse to authenticate.

## Settings UI

The goal is "feels built-in", and for KDE that concretely means a KCM (System
Settings module) sitting next to the fingerprint one, doing: enroll, list/delete
templates, enable/disable per service, run diagnostics.

This is also where the D-Bus question from [04](04-architecture.md#ipc-what-and-why)
bites — KDE's settings modules talk D-Bus and polkit natively. Writing a KCM against
a bespoke Unix socket is swimming upstream.

**Scope note:** the KCM is a separate deliverable (QML + C++/Kirigami), not part of
the Rust core, and not v1. `faciolockctl` covers everything functionally first.

## Packaging plan

Binary packages:

| Package | Contents |
|---|---|
| `faciolock` | daemon, CLI, systemd units, config |
| `libpam-faciolock` | `pam_faciolock.so` + `/usr/share/pam-configs/faciolock` |
| `faciolock-models` | the ONNX files + their licenses |
| `plasma-faciolock-kcm` | KDE settings module — **built in Phase 8**, see below |

Splitting the models out matters: it keeps the license accounting clean, lets a
security update to the daemon ship without re-shipping 37 MB, and lets someone swap
in a different model package.

### One build-dependency, and a note for whoever writes `debian/rules`

`libpam-faciolock` needs **`libpam0g-dev`** in `Build-Depends`. The module declares
libpam's four functions itself rather than taking a crate for them — that is what keeps
`cargo tree -p faciolock-pam` at exactly `libc` + `faciolock-ipc`, which is the claim
[04](04-architecture.md#crate-layout) makes and Phase 3 asserts in a test — but it still
links against libpam so the `.so` carries a `DT_NEEDED` for it. Without one, the module
resolves `pam_get_user` from whatever the loading application happened to put in the global
symbol scope, which is *usually* libpam and is not guaranteed to be.

`crates/faciolock-pam/build.rs` passes `-l:libpam.so.0` rather than `-lpam`, because
`-lpam` needs the `libpam.so` development symlink and the reference machine does not have
`libpam0g-dev` installed. A package build does, so `debian/rules` may set the linker
argument to plain `-lpam`; both produce the same `DT_NEEDED: libpam.so.0`. Nothing else
about the module has a build-time dependency.

Rust in Debian is the known-annoying part: the archive wants `librust-*-dev` source
packages for every crate, or an explicit vendoring exception. Realistic sequencing:

1. **PPA first.** Get real users on it. Nothing gets into the archive without a
   track record.
2. **Debian ITP** once the design is stable and the security story is documented.
3. **Ubuntu/Kubuntu** syncs from Debian; going straight to Ubuntu is the harder path.

### As built, 2026-08-31 (P7-b)

Three packages, not four — `plasma-faciolock-kcm` was the "(later)" row in the table above
and had no v1 deliverable behind it. **Phase 8 built it; the fourth package arrives in
`0.1.0~ppa2`.** Everything below about vendoring, `3.0 (native)` and the offline builders is
unchanged by it, with one addition: the KCM is C++ and CMake, so its build dependencies must
be *archive-available* rather than vendored. There is no `cargo vendor` for `libkf6kcmutils-dev`.

**Launchpad's builders have no network**, which is the constraint that decided the shape.
Every crate the workspace needs must be inside the source package before upload, and
`dh-cargo` — the archive's answer — wants a `librust-*-dev` for each of 156 crates. That is
step 2 of the sequence above, not step 1. So the source package carries a vendored tree,
and `tools/mk-source-package.sh` assembles it:

```text
git ls-files  ──┐
                ├─→  build copy  ──→  cargo vendor  ──→  dpkg-buildpackage
models/*.onnx ──┘
```

The models are copied in explicitly because `.gitignore` has excluded them since Phase 0
and `faciolock-models` cannot be built without them. Everything else follows from what git
tracks, which is why `corpus/` (554 MB), `spikes/` (2.5 GB) and `fuzz/` (512 MB) stay out
of the tarball without anyone having to remember them — and `fuzz/` staying out is not
housekeeping, it is the same rule that keeps `libfuzzer-sys` out of the lockfile that
`faciolock-pam`'s dependency-tree test rests on.

`vendor/` is ~194 MB and never enters the repository. Format is `3.0 (native)`: there is no
upstream/downstream split to model, so one tarball beats three files kept in step.

Three deliberate divergences from `tools/install-local.sh`, which is otherwise the file
layout spec:

| | `install-local.sh` | The package |
|---|---|---|
| Units | `/etc/systemd/system` | `/usr/lib/systemd/system` — an admin's `/etc` is not a package's to write |
| Stripping | `install -s` | `dh_strip`, which also produces `-dbgsym` packages |
| Config file | never created | never created — the same rule, and for the same reason: `probe::write_config` uses `create_new(true)` (P5-e), so a shipped conffile makes `diagnose --write` refuse forever |

The `faciolock-auth` group membership a package cannot grant, because which humans may ask
the daemon about themselves is an administrator's decision. It is in `README.Debian` and on
the VM exit path; without it, the lock screen falls through to the password field silently.

#### The multi-arch fields, taken from `fprintd` rather than reasoned about

`libpam-faciolock` is `Multi-Arch: same` — a PAM module lives in a multiarch path and an
i386 application on an amd64 system needs the i386 module. But a `Multi-Arch: same` package
may not depend on one that declares nothing, and `lintian` says so
(`dependency-is-not-multi-archified`).

`fprintd` is the design this copies and it had already answered it, on this machine:

```text
libpam-fprintd  Multi-Arch: same     Depends: fprintd (= 1.94.5-4), …
fprintd         Multi-Arch: foreign
```

So `faciolock` and `faciolock-models` are `Multi-Arch: foreign`. That is the correct claim
rather than the convenient one: what the module needs from the daemon is *a daemon on the
other end of a socket*, and the architecture of the process behind it is not the module's
business — which is the same privilege-split argument [04](04-architecture.md) makes, showing
up in a control file.

#### The two units want opposite treatment from `dh_installsystemd`

A first draft passed `--no-start` for both, on the reasonable-sounding theory that a face
authentication daemon should not begin running because you installed it. That reasoning is
right about the **service** and wrong about the **socket**, and getting it wrong fails the
phase's own exit criterion on the first command after the install:

| Unit | Treatment | Why |
|---|---|---|
| `faciolock.socket` | default — enable, start, restart after upgrade | The socket *is* the idle state. Nothing runs behind it, and `--no-start` leaves it enabled but not listening until the next reboot, so `faciolockctl diagnose` stats a socket that does not exist. The restart-after-upgrade half matters too: it is the only thing that rebinds a changed `SocketMode` or `SocketGroup`, which is the same trap `install-local.sh` guards with a deliberate `systemctl restart` |
| `faciolock.service` | `--no-start` | Starting `faciolockd` directly hands it no inherited fd, so it self-binds — and then the socket unit's inode is the one nothing is listening on |

`dh_installsystemd` also runs once per binary package and errors on a unit the current
package does not ship, so both calls are scoped with `-pfaciolock` and followed by
`--remaining-packages`.

## Desktop compatibility

Distinct from [15](15-hardware-compatibility.md), which is about cameras. This table is about
lock screens, and it has the same rule: a row says what was *observed*, or it says untested.

| Desktop | Display manager | Lock screen | Face unlock |
|---|---|---|---|
| Plasma 6.6.5 | SDDM | `kscreenlocker_greet` | **observed twice** — 2026-08-31 19:20:33 hand-installed, 23:02:00 packaged. Both journal lines carry `pid … uid 1000`, an unprivileged session process asking about itself |
| GNOME | GDM | `gnome-shell` | **untested.** Nothing is installed and nothing is claimed |

### Why GNOME is untested rather than "probably fine" (Phase 8)

It would have been easy to add `gdm-password` to a table and call it support. Two things
decide whether it works and neither can be answered without GDM on a machine, so both are
written down instead of guessed:

1. **Does `/etc/pam.d/gdm-password` include `common-auth`?** `pam-auth-update` edits
   `common-auth` and `common-account` and nothing else, which is the whole reason one profile
   reaches all six services in the table above ([data/pam-configs/README.md](../data/pam-configs/README.md)
   checked this by reading the six files, not by assuming). If GDM's stack includes it, the
   code change for GNOME support is **zero**. If it does not, the profile needs a service that
   `pam-auth-update` cannot reach and the answer is a different shape entirely.

2. **What euid does GDM's PAM conversation run as?** `serve::may_ask` is
   `peer_uid == 0 || (subject == Some(peer_uid) && matches!(op, Op::Auth | Op::Status))` —
   root may ask about anybody, anybody else may ask about themselves, and there is no third
   case. If `gdm-session-worker` runs as root the module is answered like `sudo` is. If the
   greeter asks as the `gdm` system user it is structurally refused, because `gdm` is neither
   root nor the subject — the same wall [Q10](09-roadmap.md#the-lock-screen-cannot-reach-the-socket-either-q10)
   measured for `kscreenlocker_greet`, and one that adding `gdm` to `faciolock-auth` does not
   climb, since the group only buys the right to ask about *yourself*.

The commands are `cat /etc/pam.d/gdm-password` and watching the euid of `gdm-session-worker`
during a login. Naming them costs nothing now and is most of the work later.

## The upstreaming checklist

A distro security team will ask these. Having answers *before* filing anything is
most of the work:

- [x] **A licence at all** — GPL-3.0-or-later (P7-a). This was not on the list and blocked
      every other item on it
- [~] Every shipped file DFSG-compatible, models included, licenses vendored — the shipped
      files are clean and `debian/copyright` names them per file. **Two things are open, and
      both are ITP blockers rather than PPA blockers.** SFace's provenance (P7-c): the
      Apache-2.0 grant beside the weights covers the artefact, not the question of whether
      the redistributor could grant it — written into the copyright file rather than left in
      the wiki, because that is where a reviewer looks. And `vendor/libloading-*/tests/`
      carries **two prebuilt Windows DLLs**, which nothing here builds, links or runs, and
      which are still prebuilt binaries without source. Overridden with the reason attached;
      the real fix is `Files-Excluded` repacking plus regenerating that crate's
      `.cargo-checksum.json`, and it is not worth doing before the archive path is walked
- [~] Reproducible builds — **built twice, all five artefacts byte-identical**, `-dbgsym`
      included. `SOURCE_DATE_EPOCH` from the changelog, `--locked`, vendored sources. Not
      yet path-independent: both runs used the same build directory and nothing sets
      `--remap-path-prefix`, so this is "reproducible here", not "reproducible anywhere"
- [x] `systemd-analyze security` score documented — **0.6**, and the same run does the real
      work rather than just scoring
- [x] Threat model published ([03](03-pam-and-security.md))
- [ ] Independent security review, ideally by someone who has reviewed PAM modules
- [~] Fuzzing on both the IPC parser and the frame-handling path — targets written, and
      **never run**: `cargo-fuzz` needs nightly and the reference machine has stable. The
      same properties are asserted by seeded generators in `cargo test`, which is the
      property without the coverage-guided search
- [x] `cargo tree -p faciolock-pam` = libc + ipc, asserted in CI
- [x] Documented, tested recovery path if the module misbehaves — walked on 2026-08-31
      (by-hand `sudo` line, restored from backup; then the profile, enabled and removed),
      and it now ships as `libpam-faciolock`'s `README.Debian` rather than living only here
- [x] Opt-in by default — `Default: no`, and installing the module is not enabling it
- [x] Honest README about the security ceiling — and it grew two entries in Phase 7 rather
      than losing any: B24's print and B18's daylight
- [x] CVE contact / security policy — `SECURITY.md`, with an out-of-scope section that
      names the known limitations so a reporter finds them before writing

## Hardware reality check

This depends on an IR camera that most desktops don't have. Positioning matters:
this is a **laptop** feature, on the subset with Windows Hello hardware. Realistic
first-wave targets: ThinkPad, Dell Latitude/XPS, HP EliteBook. A hardware
compatibility list in the wiki, built from real reports, will be worth more to
adoption than any amount of optimisation.

## Sources

- [pam-auth-update(8)](https://manpages.debian.org/man/pam-auth-update)
- [Ubuntu PAMConfigFrameworkSpec](https://wiki.ubuntu.com/PAMConfigFrameworkSpec)
- [KScreenLocker MR !163 — run multiple PAM sessions at once](https://invent.kde.org/plasma/kscreenlocker/-/merge_requests/163)
- [KScreenLocker MR !15 — finger unlocking](https://invent.kde.org/plasma/kscreenlocker/-/merge_requests/15)
- [KDE/kscreenlocker](https://github.com/KDE/kscreenlocker)
