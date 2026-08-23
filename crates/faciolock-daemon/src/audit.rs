//! One structured line per attempt (wiki/03 R9).
//!
//! ## Why stderr and not `libsystemd`
//!
//! systemd already captures a service's stderr into the journal, and it parses a leading
//! `<N>` — the syslog priority, the same convention `sd-daemon.h` documents — off the front
//! of each line. So the priority in `journalctl -p` costs three characters, and the
//! alternative costs a native dependency in the process that parses attacker-controlled
//! pixels. ponytail: three characters. Structured `_FIELDS` via the native protocol would
//! need `sd_journal_send`, and nothing yet needs to *query* by field.
//!
//! ## One construction site
//!
//! Every path through `Daemon::authenticate` that produces an answer emits exactly one line
//! — [`attempt`] for the paths that reached a verdict or a refusal, [`not_enrolled`] for the
//! one that has no verdict to report. That is the same discipline as the single
//! `PAM_SUCCESS` site (wiki/09) and the single `Allow` site (`serve::to_wire`), applied to
//! the log: an attempt that returned without a record would be an attempt nobody could audit.
//!
//! A **cancelled** request is the one thing that does not produce a line here. It has its own
//! log line at info and no answer goes back, because nobody is waiting for one — and it is
//! not an attempt, so counting or auditing it as one would be wrong in the same way.
//! `Op::Status` is not logged either: it is a probe that touches nothing and answers whether
//! a template exists, and logging it would bury the attempts in it.
//!
//! What is logged here is the **detailed** verdict — which check rejected, at what value.
//! What crosses the socket stays narrow, because telling a caller which check rejected them
//! is telling an attacker what to fix (wiki/04).

use faciolock_vision::{Outcome, Verdict};

use crate::ratelimit::Decision;

/// Syslog priorities, as systemd parses them off a stderr line.
const ERR: u8 = 3;
const WARNING: u8 = 4;
const NOTICE: u8 = 5;
const INFO: u8 = 6;

/// One authentication attempt, whatever came of it.
///
/// `refused` is `Some` when the rate limiter answered before any biometric work happened; in
/// that case there is no `Outcome` to describe, and the line says so rather than inventing
/// one.
pub fn attempt(
    uid: u32,
    peer: &str,
    elapsed_ms: u128,
    outcome: Option<&Outcome>,
    refused: Option<Decision>,
) {
    let (priority, body) = attempt_line(uid, peer, elapsed_ms, outcome, refused);
    emit(priority, &body);
}

/// Split from [`attempt`] so the line can be asserted rather than only printed.
///
/// `eprintln!` is not capturable from a test, and the shape of this string is not cosmetic:
/// `tools/ratelimit-try.sh` greps it, and so will whoever is reading a journal at the point
/// they most need it to parse.
fn attempt_line(
    uid: u32,
    peer: &str,
    elapsed_ms: u128,
    outcome: Option<&Outcome>,
    refused: Option<Decision>,
) -> (u8, String) {
    let measured = match (outcome, refused) {
        (_, Some(decision)) => {
            let priority = match decision {
                Decision::LockedOut => ERR,
                _ => WARNING,
            };
            return (
                priority,
                format!(
                    "auth uid={uid} verdict=RateLimited reason={} {peer} elapsed={elapsed_ms} counted=0",
                    decision.reason()
                ),
            );
        }
        (Some(outcome), None) => outcome,
        // Nothing measured and nothing refused: the store failed, or the camera did. The
        // caller still gets an answer, so the journal still gets a line.
        (None, None) => {
            return (
                INFO,
                format!(
                    "auth uid={uid} verdict=Unavailable reason=nothing-measured {peer} elapsed={elapsed_ms} counted=0"
                ),
            );
        }
    };

    let counted = u8::from(measured.verdict.counts_as_an_attempt());
    (
        priority_of(&measured.verdict),
        format!(
            "auth uid={uid} verdict={} {} live={} probes={} {peer} elapsed={elapsed_ms} counted={counted}",
            name_of(&measured.verdict),
            detail_of(&measured.verdict),
            liveness_of(measured),
            measured.probes,
        ),
    )
}

/// A request for a user with no template.
///
/// Its own function rather than a `Verdict` arm because there is no verdict for it — the
/// pipeline never ran, so there is no `Outcome` to describe and nothing measured to report.
/// Logged at info: on a machine where the module is installed and nobody has enrolled, this
/// is every lock screen and every `sudo`, and putting that at warning would train an
/// operator to ignore the level. R9 wants the line; it does not want it shouted.
pub fn not_enrolled(uid: u32, peer: &str, elapsed_ms: u128) {
    emit(
        INFO,
        &format!(
            "auth uid={uid} verdict=NoEnrolment reason=no-template {peer} elapsed={elapsed_ms} counted=0"
        ),
    );
}

/// A cleared lockout is a security-relevant state change and gets its own line. Only when
/// something was actually cleared — `acct_mgmt` fires on every successful login.
pub fn cleared(uid: u32, peer: &str) {
    emit(NOTICE, &format!("clear uid={uid} event=lockout-cleared {peer}"));
}

/// `<N>` is the syslog priority; systemd strips it off stderr and the rest is the line.
/// The caller supplies its own event word, so `journalctl … | grep 'faciolockd: auth'`
/// selects attempts and nothing else.
fn emit(priority: u8, body: &str) {
    eprintln!("<{priority}>faciolockd: {body}");
}

fn priority_of(verdict: &Verdict) -> u8 {
    match verdict {
        Verdict::Match { .. } => NOTICE,
        Verdict::NoMatch { .. } | Verdict::NotLive { .. } | Verdict::NoFace => WARNING,
        // Not failures. A bright room or a busy camera is the daemon declining to answer,
        // and logging those at warning would train an operator to ignore the level.
        Verdict::Environment { .. } | Verdict::Unusable(_) => INFO,
        Verdict::Failed(_) => ERR,
    }
}

/// The wire name, not the Rust one, so a journal line and a PAM debug line agree.
fn name_of(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Match { .. } => "Allow",
        Verdict::NoMatch { .. } | Verdict::NotLive { .. } | Verdict::NoFace => "Deny",
        Verdict::Environment { .. } | Verdict::Unusable(_) | Verdict::Failed(_) => "Unavailable",
    }
}

/// R9's "reason, liveness score, match distance", one `key=value` group per verdict shape.
fn detail_of(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Match { similarity, agreeing } => {
            format!("reason=match similarity={similarity:.3} agreeing={agreeing}")
        }
        Verdict::NoMatch { best } => format!("reason=no-match best={best:.3}"),
        Verdict::NotLive { check, worst, threshold } => {
            format!("reason=not-live check={check:?} worst={worst:.4} threshold={threshold:.4}")
        }
        Verdict::NoFace => "reason=no-face".into(),
        Verdict::Environment { ambient_floor } => {
            format!("reason=ambient floor={ambient_floor:.2}")
        }
        Verdict::Unusable(e) => format!("reason=unusable detail={e:?}"),
        // Quoted: this is the one field carrying a free-form string, and an unquoted space
        // in it would silently split into two bogus keys.
        Verdict::Failed(e) => format!("reason=failed detail={:?}", e),
    }
}

/// Every check that failed, enforced or advisory, or `pass`.
///
/// Advisory failures are included deliberately: they are the checks waiting on a spoof
/// recording to calibrate against, and the journal is where the evidence for calibrating
/// them accumulates. An advisory failure did not reject anything, and the `~` marks it.
fn liveness_of(outcome: &Outcome) -> String {
    let mut failed: Vec<String> = outcome
        .liveness
        .results
        .iter()
        .filter(|r| !r.passed())
        .map(|r| {
            let mark = if r.rejects() { "" } else { "~" };
            format!("{mark}{:?}:{:.4}", r.check, r.worst)
        })
        .collect();
    if failed.is_empty() {
        return "pass".into();
    }
    failed.sort();
    failed.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use faciolock_vision::{BurstError, Check, CheckResult, Enforcement, LivenessReport};

    fn outcome(verdict: Verdict, results: Vec<CheckResult>) -> Outcome {
        Outcome {
            verdict,
            liveness: LivenessReport { results, pairs: 3 },
            face: None,
            probes: 3,
        }
    }

    /// The wire name is the narrow one and it must agree with `serve::to_wire`, or the
    /// journal and the caller describe the same event differently.
    #[test]
    fn the_logged_name_matches_what_went_over_the_wire() {
        for verdict in [
            Verdict::Match { similarity: 0.9, agreeing: 2 },
            Verdict::NoMatch { best: 0.31 },
            Verdict::NotLive { check: Check::DarkFrame, worst: 1.0, threshold: 1e-3 },
            Verdict::NoFace,
            Verdict::Environment { ambient_floor: 51.74 },
            Verdict::Unusable(BurstError::Malformed),
            Verdict::Failed("camera vanished".into()),
        ] {
            let wire = crate::serve::to_wire(&verdict);
            assert_eq!(
                name_of(&verdict),
                format!("{wire:?}"),
                "{verdict:?} is described differently in the journal and on the wire"
            );
        }
    }

    /// Only a match is a notice; a failure of the pipeline itself is an error. If everything
    /// shared a priority the level would carry no information.
    #[test]
    fn priorities_separate_a_refusal_from_a_fault() {
        assert_eq!(priority_of(&Verdict::Match { similarity: 0.9, agreeing: 2 }), NOTICE);
        assert_eq!(priority_of(&Verdict::NoFace), WARNING);
        assert_eq!(priority_of(&Verdict::Environment { ambient_floor: 51.7 }), INFO);
        assert_eq!(priority_of(&Verdict::Failed("x".into())), ERR);
    }

    /// R9 wants the liveness detail, and an advisory failure must be distinguishable from
    /// the enforced one that actually rejected the burst.
    #[test]
    fn advisory_failures_are_logged_and_marked_as_not_having_rejected() {
        let enforced = CheckResult {
            check: Check::DarkFrame,
            worst: 0.664,
            threshold: 0.001,
            enforcement: Enforcement::Enforced,
            failing_pairs: 2,
        };
        let advisory = CheckResult {
            check: Check::Falloff,
            worst: 0.04,
            threshold: 0.05,
            enforcement: Enforcement::Advisory,
            failing_pairs: 1,
        };
        let passing = CheckResult { failing_pairs: 0, ..advisory };

        let both = outcome(Verdict::NoFace, vec![enforced, advisory]);
        let text = liveness_of(&both);
        assert!(text.contains("DarkFrame:0.6640"), "the enforced failure is named: {text}");
        assert!(text.contains("~Falloff"), "an advisory failure is marked: {text}");

        assert_eq!(liveness_of(&outcome(Verdict::NoFace, vec![passing])), "pass");
        assert_eq!(liveness_of(&outcome(Verdict::NoFace, vec![])), "pass");
    }

    /// Every field R9 names has to be in the line. Asserted on the formatted detail rather
    /// than on the `eprintln`, which a test cannot read back.
    #[test]
    fn the_detail_carries_the_number_the_verdict_turned_on() {
        assert!(detail_of(&Verdict::NoMatch { best: 0.3124 }).contains("best=0.312"));
        assert!(
            detail_of(&Verdict::Match { similarity: 0.8765, agreeing: 2 })
                .contains("similarity=0.876")
        );
        let live = detail_of(&Verdict::NotLive {
            check: Check::Falloff,
            worst: 0.2661,
            threshold: 0.05,
        });
        assert!(live.contains("check=Falloff") && live.contains("worst=0.2661"), "{live}");
    }

    /// **The whole line, as a journal would carry it.** R9's field list is the spec and
    /// `tools/ratelimit-try.sh` greps this shape, so a format string edited without the
    /// harness fails here rather than in front of a lock screen.
    #[test]
    fn the_assembled_line_carries_every_field_r9_asks_for() {
        let peer = "pid 27608 uid 0";
        let denied = outcome(Verdict::NoMatch { best: 0.3124 }, vec![]);
        let (priority, body) = attempt_line(1000, peer, 673, Some(&denied), None);

        assert_eq!(priority, WARNING);
        assert_eq!(
            body,
            "auth uid=1000 verdict=Deny reason=no-match best=0.312 live=pass probes=3 \
             pid 27608 uid 0 elapsed=673 counted=1"
        );

        // Every field R9 names, present by name rather than by position.
        for field in ["uid=", "verdict=", "reason=", "elapsed=", "counted=", "live="] {
            assert!(body.contains(field), "{field} missing from {body}");
        }
    }

    /// The two refusal shapes, which have no `Outcome` behind them and are what the harness
    /// asserts on. `counted=` is load-bearing: R5's exemptions are a policy worth arguing
    /// with, and a log that hid which way it went would make that unresolvable.
    #[test]
    fn a_refusal_produces_a_countable_line_at_the_right_level() {
        let (priority, body) =
            attempt_line(1000, "pid 1 uid 0", 0, None, Some(Decision::LockedOut));
        assert_eq!(priority, ERR, "a lockout is an err-level event");
        assert_eq!(
            body,
            "auth uid=1000 verdict=RateLimited reason=lockout pid 1 uid 0 elapsed=0 counted=0"
        );

        let backoff = Decision::Backoff { retry_in: std::time::Duration::from_secs(1) };
        let (priority, body) = attempt_line(1000, "pid 1 uid 0", 0, None, Some(backoff));
        assert_eq!(priority, WARNING, "a backoff is not a lockout");
        assert!(body.contains("reason=backoff") && body.ends_with("counted=0"), "{body}");
    }

    /// A free-form failure string must not split into bogus keys — it is the one field
    /// carrying arbitrary text, and it comes from error paths nobody controls the wording of.
    #[test]
    fn a_free_form_failure_message_stays_one_field() {
        let text = detail_of(&Verdict::Failed("camera vanished mid burst".into()));
        assert!(text.contains(r#"detail="camera vanished mid burst""#), "{text}");
        assert_eq!(text.split(' ').filter(|w| w.contains('=')).count(), 2, "{text}");
    }
}
