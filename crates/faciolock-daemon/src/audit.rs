// ponytail: plain text, not _FIELDS, see wiki/14

use faciolock_vision::{Outcome, Verdict};

use crate::ratelimit::Decision;

const ERR: u8 = 3;
const WARNING: u8 = 4;
const NOTICE: u8 = 5;
const INFO: u8 = 6;

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

pub fn not_enrolled(uid: u32, peer: &str, elapsed_ms: u128) {
    emit(
        INFO,
        &format!(
            "auth uid={uid} verdict=NoEnrolment reason=no-template {peer} elapsed={elapsed_ms} counted=0"
        ),
    );
}

pub fn cleared(uid: u32, peer: &str) {
    emit(NOTICE, &format!("clear uid={uid} event=lockout-cleared {peer}"));
}

pub fn enrol_pose(uid: u32, peer: &str, elapsed_ms: u128, poses: usize, refused: Option<&Verdict>) {
    let (priority, reason) = match refused {
        None => (INFO, "reason=captured".to_string()),
        Some(v) => (priority_of(v), detail_of(v)),
    };
    emit(
        priority,
        &format!(
            "enrol uid={uid} pose={poses}/{} {reason} {peer} elapsed={elapsed_ms}",
            faciolock_ipc::ENROL_POSES
        ),
    );
}

pub fn enrolled(uid: u32, peer: &str, embeddings: usize) {
    emit(
        NOTICE,
        &format!(
            "enrol uid={uid} event=template-written poses={} embeddings={embeddings} {peer}",
            faciolock_ipc::ENROL_POSES
        ),
    );
}

fn emit(priority: u8, body: &str) {
    eprintln!("<{priority}>faciolockd: {body}");
}

fn priority_of(verdict: &Verdict) -> u8 {
    match verdict {
        Verdict::Match { .. } => NOTICE,
        Verdict::NoMatch { .. } | Verdict::NotLive { .. } | Verdict::NoFace => WARNING,
        Verdict::Environment { .. } | Verdict::Unusable(_) => INFO,
        Verdict::Failed(_) => ERR,
    }
}

fn name_of(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Match { .. } => "Allow",
        Verdict::NoMatch { .. } | Verdict::NotLive { .. } | Verdict::NoFace => "Deny",
        Verdict::Environment { .. } | Verdict::Unusable(_) | Verdict::Failed(_) => "Unavailable",
    }
}

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
        Verdict::Failed(e) => format!("reason=failed detail={:?}", e),
    }
}

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

    #[test]
    fn priorities_separate_a_refusal_from_a_fault() {
        assert_eq!(priority_of(&Verdict::Match { similarity: 0.9, agreeing: 2 }), NOTICE);
        assert_eq!(priority_of(&Verdict::NoFace), WARNING);
        assert_eq!(priority_of(&Verdict::Environment { ambient_floor: 51.7 }), INFO);
        assert_eq!(priority_of(&Verdict::Failed("x".into())), ERR);
    }

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

        for field in ["uid=", "verdict=", "reason=", "elapsed=", "counted=", "live="] {
            assert!(body.contains(field), "{field} missing from {body}");
        }
    }

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

    #[test]
    fn a_free_form_failure_message_stays_one_field() {
        let text = detail_of(&Verdict::Failed("camera vanished mid burst".into()));
        assert!(text.contains(r#"detail="camera vanished mid burst""#), "{text}");
        assert_eq!(text.split(' ').filter(|w| w.contains('=')).count(), 2, "{text}");
    }
}
