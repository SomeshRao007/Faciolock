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
| GDM/GNOME (later) | `gdm-password` | for portability beyond Kubuntu |

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

### The single line, and what makes it safe

Back the file up first — the undo below depends on it existing:

```bash
sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.faciolock-backup
```

```
auth	[success=end default=ignore]	pam_faciolock.so
```

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
| `plasma-faciolock-kcm` (later) | KDE settings module |

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

## The upstreaming checklist

A distro security team will ask these. Having answers *before* filing anything is
most of the work:

- [ ] Every shipped file DFSG-compatible, models included, licenses vendored
- [ ] Reproducible builds
- [ ] `systemd-analyze security` score documented
- [ ] Threat model published ([03](03-pam-and-security.md))
- [ ] Independent security review, ideally by someone who has reviewed PAM modules
- [ ] Fuzzing on both the IPC parser and the frame-handling path (`cargo-fuzz`)
- [ ] `cargo tree -p faciolock-pam` = libc + ipc, asserted in CI
- [ ] Documented, tested recovery path if the module misbehaves (boot to recovery, `pam-auth-update`)
- [ ] Opt-in by default
- [ ] Honest README about the security ceiling
- [ ] CVE contact / security policy

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
