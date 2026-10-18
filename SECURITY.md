# Security Policy

## Reporting a vulnerability

**Do not open a public issue.**

Report it privately through GitHub's
[private vulnerability reporting](https://github.com/SomeshRao007/Faciolock/security/advisories/new).

Include what you observed, how to reproduce it, the package version
(`dpkg -s faciolock | grep Version`), and the hardware if it is relevant. A proof of concept is welcome
but not required.

This is a single-maintainer project with no dedicated security team. The timelines
below are what one maintainer can reliably commit to:

| Stage | Target |
|---|---|
| Acknowledgement | 7 days |
| Initial assessment, with a severity | 14 days |
| Fix, or a stated reason there will not be one | 90 days |
| Public disclosure | after the fix ships, or at 90 days, whichever is first |

If you intend to disclose on your own schedule, say so in the first report and we will
work to it. Coordinated disclosure is preferred; it is not a condition of being credited
in the release notes.

## Supported versions

Only the latest release.

## What is in scope

The daemon, the PAM module, the CLI, the IPC protocol between them, the systemd units
and sandbox profile, the template store and its encryption, the rate limiting, and the
packaging.

Concretely, a report should be about:

- Anything that authenticates a user who is not the enrolled user
- Anything that lets an unprivileged process read, replace or exfiltrate a template
- Anything that lets an unprivileged process forge, replay or tamper with an IPC verdict
- Anything that escapes the daemon's sandbox
- Anything that makes the PAM module return success on a path that is not a match
- Anything that lets a process reset another user's failure budget

## What is out of scope

**These are known and documented limitations, not vulnerabilities.**

- **A printed photograph is not reliably rejected.** A flat print reads above the worst
  live face on the falloff statistic the liveness check uses, so there is currently no
  enforced check that rejects a print.
- **A purpose-built 3D mask under active NIR illumination is not reliably rejected.**
  This is the same bar Windows Hello sits at.
- **The false accept rate is unmeasured.** The cosine threshold is OpenCV's published
  0.363, calibrated on RGB data. A report that the threshold is wrong is welcome as an
  *issue*; it is not a vulnerability report, because the project has never claimed a
  measured FAR.
- **Anything running as your user can spend your failure budget.** By design: the socket
  is group-gated so that a session process can ask about *itself*, which is what makes
  the lock screen work at all. `fprintd` sits in exactly the same place.
- **An attacker with root can do anything.** The threat model does not include a
  hostile root.
- **Physical attacks on an unlocked machine.** Out of scope for the same reason.
- **Face authentication refusing to work.** A refusal falls through to your password by
  construction. The design deliberately prefers refusing you to admitting somebody else.

## The security ceiling

Face authentication is a **convenience factor, not a password replacement**. Faciolock
is designed to sit alongside your password and never to replace it, and a password stays
reachable on every path.

If you need a second factor that resists a motivated attacker, use a hardware token.

## Review status

No independent security review has been done yet.
