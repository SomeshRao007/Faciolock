# Security Policy

## Reporting a vulnerability

**Do not open a public issue.**

Email **cokasomeshrao@outlook.com** with `[faciolock security]` in the subject.

Include what you need to include and nothing you do not: what you observed, how to
reproduce it, the version (`faciolockctl --version` or the package version), and the
hardware if it is relevant. A proof of concept is welcome and not required.

You will get an acknowledgement within **7 days**. If you do not, assume the mail was
lost and send it again — a silent report is worse for both of us than a duplicate.

This is a one-person project with no security team behind it. The timelines below are
what one maintainer can honestly promise, not what a vendor with a rota would:

| Stage | Target |
|---|---|
| Acknowledgement | 7 days |
| Initial assessment, with a severity | 14 days |
| Fix or a stated reason there will not be one | 90 days |
| Public disclosure | after the fix ships, or at 90 days, whichever is first |

If you intend to disclose on your own schedule, say so in the first mail and we will
work to it. Coordinated disclosure is preferred; it is not a condition of being
listed in the fix's release notes.

## Supported versions

Only the latest release. There is no LTS branch and pretending otherwise would be a
promise this project cannot keep.

## What is in scope

The daemon, the PAM module, the CLI, the IPC protocol between them, the systemd units
and sandbox profile, the template store and its encryption, the rate limiting, and the
packaging.

Concretely, these are the things a report should be about:

- Anything that authenticates a user who is not the enrolled user
- Anything that lets an unprivileged process read, replace or exfiltrate a template
- Anything that lets an unprivileged process forge, replay or tamper with an IPC verdict
- Anything that escapes the daemon's sandbox
- Anything that makes the PAM module return success on a path that is not a match
- Anything that lets a process reset another user's failure budget

## What is out of scope — and why

**These are known and documented limitations, not vulnerabilities.** A report of one is
a report of something already written down; please read this list before writing.

- **A printed photograph is not reliably rejected.** Measured, not suspected: a flat
  print reads above the worst live face on the falloff statistic the liveness check
  uses. There is currently **no** enforced check that rejects a print.
  ([B24](wiki/11-bug-log.md#b24))
- **A purpose-built 3D mask under active NIR illumination is not reliably rejected.**
  This is the same bar Windows Hello sits at and it is stated on the front page.
- **False accept rate is unmeasured.** The corpus has one subject. The cosine threshold
  is OpenCV's RGB-calibrated 0.363, which is paper-derived and provisional. A report
  that the threshold is wrong is welcome as an *issue*; it is not a vulnerability
  report, because the project has never claimed a measured FAR.
- **Anything running as your user can spend your failure budget.** By design, after
  [P6-d](wiki/08-open-questions.md#q10): the socket is group-gated so a session process
  can ask about *itself*, which is what makes the lock screen work at all. `fprintd`
  sits in exactly the same place.
- **An attacker with root can do anything.** Root owns the machine. The threat model is
  in [wiki/03](wiki/03-pam-and-security.md); it has never included a hostile root.
- **Physical attacks on an unlocked machine.** Out of scope for the same reason.
- **Face authentication refusing to work.** A refusal falls through to your password by
  construction. Availability is not the security property this project defends; the
  design deliberately prefers refusing you to admitting somebody else.

## The security ceiling, stated once

Face authentication is a **convenience factor, not a password replacement**. Faciolock
is designed to sit alongside your password and never to replace it, a password stays
reachable on every path, and the module ships disabled by default.

If you need a second factor that resists a motivated attacker, use a hardware token.

## Design documents

A reviewer should not have to reverse-engineer the intent from the code:

- [Threat model, PAM behaviour, the sandbox](wiki/03-pam-and-security.md)
- [Architecture and the privilege split](wiki/04-architecture.md)
- [The bug log — every defect found, including the ones still open](wiki/11-bug-log.md)

## What has and has not been reviewed

No independent security review has been done. That is an open item on the upstreaming
checklist in [wiki/07](wiki/07-desktop-and-packaging.md#the-upstreaming-checklist), and
until it is closed, this project's security claims rest on one person's testing.
