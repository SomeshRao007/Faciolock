//! Thresholds and the provenance of every one of them.
//!
//! A threshold in this project is a security parameter, and the standard wiki/05 sets
//! for itself is that it must be *measured*, not asserted. Exactly one of the liveness
//! thresholds currently meets that standard. The rest are carried here with an
//! [`Enforcement`] level that says so out loud, so a verdict can never imply a
//! guarantee the corpus does not support.

/// Whether a check's threshold is trusted enough to reject on.
///
/// This exists because three of the four liveness checks have no spoof sample to
/// calibrate against -- there is no printed photo and no screen in the corpus. Shipping
/// them silently disabled would look complete and defend nothing; shipping them with
/// invented thresholds is what [B9] proved is unpredictable. Reporting the level per
/// check is the honest middle, and it makes the audit log in Phase 4 truthful for free.
///
/// [B9]: ../../../wiki/11-bug-log.md
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enforcement {
    /// Calibrated against a real attack recording. May reject.
    Enforced,
    /// Computed and reported, but cannot reject on its own. No attack sample exists yet.
    Advisory,
}

/// A threshold plus how far it is trusted.
#[derive(Clone, Copy, Debug)]
pub struct Threshold {
    pub value: f64,
    pub enforcement: Enforcement,
}

impl Threshold {
    pub const fn enforced(value: f64) -> Threshold {
        Threshold { value, enforcement: Enforcement::Enforced }
    }
    pub const fn advisory(value: f64) -> Threshold {
        Threshold { value, enforcement: Enforcement::Advisory }
    }
    /// True when this threshold is allowed to turn a failing measurement into a reject.
    pub fn rejects(&self) -> bool {
        self.enforcement == Enforcement::Enforced
    }
}

/// Which illumination the hardware provides, and therefore which checks are available.
///
/// **This is enrolment-time configuration, never inferred from the burst being
/// authenticated.** Inferring it per burst would let an attacker with a bright enough
/// continuous IR source wash out the ON/OFF separation and thereby *demote* the machine
/// to a tier whose checks they can pass. A burst that does not conform to the configured
/// tier is a reject, not a downgrade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Self-strobing IR. Full liveness. The reference machine.
    IrStrobe,
    /// IR camera, emitter steady. Passive checks only -- none of which are built yet.
    IrConstant,
    /// No IR camera. Howdy's security level; requires explicit opt-in and warns loudly.
    Rgb,
}

#[derive(Clone, Copy, Debug)]
pub struct LivenessConfig {
    /// Reject when `max(mean(OFF)/mean(ON))` over the burst exceeds this.
    ///
    /// **The one verified threshold in the project.** Measured over 210 live pairs with
    /// zero false rejects; worst honest capture read 1.54e-5 and the floodlight attack
    /// 1.30e-1, four orders of magnitude apart. 1e-3 sits ~65x above the worst live
    /// reading and ~130x below the attack. wiki/11 B9.
    pub dark_ratio_max: Threshold,

    /// Reject when the ON/OFF response falls below this.
    ///
    /// Advisory, and **measurement refuted the reason it was supposed to be enforceable**
    /// (wiki/11 B17). This was the check meant to reject a phone or laptop screen, on the
    /// grounds that an LCD or OLED emits essentially nothing at 850 nm so D collapses to
    /// ~0. A phone at maximum brightness showing a frontal photo, recorded with a
    /// same-session control:
    ///
    /// ```text
    /// live (5 captures)     48.58 - 75.48
    /// spoof_phone                   36.17   <- predicted ~0
    /// attack_floodlight             37.29
    /// null_empty_room               55.53   <- an empty room outscores several live faces
    /// ```
    ///
    /// The screen is dark in NIR; the statistic is the problem. Pre-box D is computed over
    /// the **whole frame**, which is mostly room, so it measures the wall behind the phone.
    /// The `null_empty_room` row said so from the beginning.
    ///
    /// The phone is still rejected end to end — as `NoFace`, by detection, because YuNet
    /// finds nothing in a screen under NIR. But that is not this check working, and the
    /// residual 1.34x gap between 36.17 and 48.58 is far too thin to enforce on one sample
    /// of one phone in one room. 5.0 is ~10x below the weakest live reading and rejects
    /// only a total absence of illumination.
    ///
    /// What would work is D restricted to the detected face box, which exists only when
    /// detection succeeds — a brighter laptop panel is the untested case, and `spoof_laptop`
    /// is now the recording that matters most after the print.
    pub response_min: Threshold,

    /// Reject when the spatial falloff of D across the face box is *below* this --
    /// i.e. when the response is suspiciously uniform.
    ///
    /// Deliberately a **degenerate-flatness floor, not a separating threshold**.
    ///
    /// A live face is a curved diffuse reflector so D varies strongly across it; a flat
    /// print reflects near-uniformly. The physics is sound and the arithmetic is
    /// unit-tested against synthetic domed-versus-flat data, but the *separation* between
    /// a real face and a real print has never been measured — wiki/09 calls that "the one
    /// genuinely unproven claim in the project" and it stays unproven until a
    /// `spoof_print_*` recording exists.
    ///
    /// So this claims only what it can prove. Live falloff CV over the **detected face
    /// box** (which is B7's closure — the Phase 0 centre crop was measuring a
    /// subject-versus-background gradient and gave 0.31 and 0.64 for the same pose):
    ///
    /// ```text
    /// live_lit_frontal     0.3130    live_lit_glasses     0.2989
    /// live_dark_frontal    0.2900    live_lit_angle_left  0.2661   <- worst
    /// ```
    ///
    /// (Measured at the 320x320 detector. At 192 the same captures read 0.26-0.39 — a
    /// wider spread, because a coarser box tracks the face less closely. Another reason
    /// the numbers here name the configuration they came from.)
    ///
    /// A perfectly flat synthetic presentation scores below 0.01. 0.05 sits ~5x under the
    /// worst honest capture and ~5x over a degenerate one: enforced, because it costs zero
    /// false rejects on everything measured, and it rejects a perfectly flat presentation.
    /// **It does not claim to reject a printed photo.** Four captures, one subject, one
    /// session — thinner evidence than B9's 65x margin, and the reason this is a floor
    /// rather than a threshold.
    pub falloff_cv_min: Threshold,

    /// Reject when consecutive OFF frames differ by more than this fraction of the
    /// pair's response.
    ///
    /// Advisory, and the statistic is **not the one wiki/05 originally specified**. B9
    /// measured the coefficient of variation of `mean(OFF)` across a burst and found it
    /// does not separate at all -- 0.54 for a pulsed attack against 0.33-0.65 for live
    /// captures -- because a live OFF frame is pure sensor read noise and read noise is
    /// relatively noisy however tiny it is absolutely. This is a per-pixel difference over
    /// the face box instead, which is a genuinely different quantity: a video replay beats
    /// against the sensor's frame rate in the *image*, not merely in the mean.
    ///
    /// **It reads 0.0000 on every live capture in the corpus**, because a true dark frame
    /// on this hardware really is black and two black frames really are identical. That is
    /// the designed behaviour and it is also why the check has demonstrated no
    /// discriminating power yet: everything it could catch has an OFF frame that is *not*
    /// black, and the corpus contains no such recording except the floodlight, which the
    /// dark-frame check already rejects outright. It needs `spoof_phone` before it is
    /// worth anything at all.
    pub motion_max: Threshold,
}

impl Default for LivenessConfig {
    fn default() -> LivenessConfig {
        LivenessConfig {
            dark_ratio_max: Threshold::enforced(1e-3),
            response_min: Threshold::advisory(5.0),
            falloff_cv_min: Threshold::enforced(0.05),
            motion_max: Threshold::advisory(1.0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DetectConfig {
    /// Side length of the square the frame is letterboxed into before YuNet runs.
    ///
    /// **320, and the 192 this started at was wrong.** Measured, not argued.
    ///
    /// A 640x360 frame letterboxed to 192 puts a laptop-distance face at ~45 px, and its
    /// landmarks then scale back up 3.3x into the alignment. The cost of that was converted
    /// into the only currency that matters — how far the embedding moves — by embedding the
    /// same frame from each size's landmarks against a 448 reference:
    ///
    /// ```text
    /// size   worst embedding cosine vs 448   YuNet
    ///  160                          0.8861    8.2 ms
    ///  192                          0.8235   11.5 ms
    ///  320                          0.9368   33.0 ms
    /// ```
    ///
    /// Against a perturbation curve measured on the same frames (1 px costs 0.993, 4 px
    /// 0.960, 8 px 0.911), running at 192 costs *more than 8 px of landmark error* — the
    /// same magnitude as the difference between two entirely separate captures of the same
    /// person, which read 0.60-0.90. That is a large piece of the genuine/impostor margin
    /// spent on detector resolution.
    ///
    /// Note 160 and 192 are not even ordered with respect to each other: the landmark
    /// regression is simply noisy at that scale rather than converging. 320 costs 22 ms
    /// more than 192 and buys back most of the loss.
    pub input_size: usize,
    /// Minimum `sqrt(cls * obj)` for a candidate to survive.
    ///
    /// Measured on the Phase 0 corpus at 192x192, not copied from OpenCV — whose default
    /// of 0.9 rejects an angled IR face outright:
    ///
    /// ```text
    /// live_lit_frontal          0.905
    /// live_dark_frontal         0.905
    /// live_lit_glasses          0.890
    /// live_lit_angle_left       0.549   <- an angled face scores far lower
    /// attack_floodlight_remote  0.078   <- noise, and an absurd 553x552 box
    /// null_empty_room           (none)
    /// ```
    ///
    /// 0.3 sits ~4x above the noise and ~1.8x below the weakest real face. Detection is
    /// not a security boundary — a spurious box still has to pass liveness and matching —
    /// so this is tuned for recall. One subject, four captures: provisional.
    pub score_min: f32,
    /// IoU above which the lower-scoring of two candidates is suppressed.
    pub nms_iou_max: f32,
}

impl Default for DetectConfig {
    fn default() -> DetectConfig {
        DetectConfig { input_size: 320, score_min: 0.3, nms_iou_max: 0.3 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MatchConfig {
    /// Cosine similarity above which an embedding is considered the enrolled user.
    ///
    /// **Provisional and paper-derived.** 0.363 is OpenCV's published operating point for
    /// SFace, calibrated on RGB face data. The corpus holds exactly one subject, so FAR
    /// cannot be measured from it at all, and no IR-mode figure exists for this model.
    /// wiki/05 requires this default be "chosen from measurement on our own corpus" and
    /// it currently is not. It must not be described as if it were.
    pub cosine_min: f64,
    /// How many embeddings must agree.
    pub agree_k: usize,
    /// How many embeddings are computed.
    ///
    /// 2-of-3 rather than 2-of-2 so a blink or a motion blur on one frame is outvoted
    /// instead of failing the attempt. Costs a third SFace pass -- 85 ms under tract --
    /// which is why wiki/06's warm path is ~403 ms. Matching aggregates k-of-n on
    /// purpose; liveness aggregates conjunctively. **They are opposite rules and
    /// unifying them reintroduces B9.**
    pub agree_n: usize,
}

impl Default for MatchConfig {
    fn default() -> MatchConfig {
        MatchConfig { cosine_min: 0.363, agree_k: 2, agree_n: 3 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub tier: Tier,
    /// Frames discarded from the head of a burst while the AGC converges. The reference
    /// machine reads 22.7, 23.4, 14.3, 31.4 before settling into a clean alternation
    /// (wiki/10 Probe 3). Pre-warm exists partly so this is paid before authentication
    /// rather than during it.
    pub agc_settle_frames: usize,
    /// Minimum ON/OFF pairs a burst must yield.
    ///
    /// Never zero. Conjunction over an empty set is *true*, so a burst that produced no
    /// pairs would sail through every liveness check that was supposed to inspect it.
    /// B9's four-orders-of-magnitude margin was also computed at two pairs; fewer has
    /// never been measured.
    pub min_pairs: usize,
    /// Ambient near-infrared above which this hardware **cannot verify liveness at all**,
    /// measured as the dimmest frame mean in the burst.
    ///
    /// Not a liveness check. A gate in front of them, and the honest answer to
    /// [B18](../../../wiki/11-bug-log.md): in bright ambient IR the emitter's contribution
    /// stops carrying the signal every check depends on, and a floodlight attack measured
    /// *more live-looking than a real face* on every statistic tried. There is no cleverer
    /// arrangement of those terms that recovers a separation which is not in the signal, so
    /// the system detects the condition and refuses instead of guessing.
    ///
    /// **Why the ambient floor and not a ratio or an SNR.** It is the only candidate an
    /// attacker can move in one direction only. Raising ambient raises the floor, which
    /// closes the gate — fail-closed. A gate keyed on emitter-to-ambient ratio or on SNR
    /// puts an attacker-raisable term where it would hold the gate *open*, which is the
    /// synchronised-injection case B9 never covered.
    ///
    /// **Where 0.04 comes from.** It is the point at which
    /// [`LivenessConfig::dark_ratio_max`] stops working, derived rather than picked. That
    /// check rejects when `mean(OFF)/mean(ON) > 1e-3`; with an ambient floor `F` and an
    /// emitter amplitude `A`, that ratio is about `F/(F+A)`, so it holds only while
    /// `F < A/999`. The weakest live amplitude measured is 48, giving **F < 0.048** — and
    /// the gate sits just under that, not on it. A gate placed *above* the break-even
    /// would admit captures the dark-frame check then false-rejects, which is the confusing
    /// failure this is meant to replace. `the_gate_closes_before_the_dark_frame_check_breaks`
    /// pins the relationship rather than the number, so re-tuning either one keeps them
    /// consistent.
    ///
    /// Measured floors either side of it:
    ///
    /// ```text
    /// dark / lamp-lit room (7 captures)   0.0001 – 0.0020    <- 25x under the gate
    /// blinds half closed                          0.92       <- gated out
    /// blinds open                          20.33 – 51.74     <- gated out
    /// ```
    ///
    /// Note the third of those includes a *midday* capture taken with the blinds closed
    /// under a ceiling light, which read 0.0000: electric light emits almost no NIR, so
    /// this gate is about light, not about the clock.
    ///
    /// **Gating half-blinds out is a real usability loss and it is accepted knowingly** —
    /// `dark_ratio_max` already rejects that capture (0.026 against its 1e-3), so the gate
    /// costs no working case that exists today. It converts a confusing biometric failure
    /// into an honest "too bright to verify".
    ///
    /// **Provisional.** One machine, one subject, and no attack recording between 0.92 and
    /// 20.33 — so the level at which attack/live separation actually dies is bracketed, not
    /// located. The ambient sweep closes that. This will end up per-machine config with a
    /// `diagnose`-time measurement behind it.
    pub ambient_floor_max: f64,
    pub liveness: LivenessConfig,
    pub detect: DetectConfig,
    pub matching: MatchConfig,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            tier: Tier::IrStrobe,
            agc_settle_frames: 5,
            min_pairs: 2,
            ambient_floor_max: 0.04,
            liveness: LivenessConfig::default(),
            detect: DetectConfig::default(),
            matching: MatchConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the enforcement level: a check may only reject on evidence the
    /// corpus actually contains. If this ever loosens, the project is claiming a defence
    /// it has not tested.
    #[test]
    fn enforcement_matches_what_the_corpus_can_support() {
        let l = LivenessConfig::default();
        assert!(l.dark_ratio_max.rejects(), "measured against a real attack, wiki/11 B9");
        assert!(
            l.falloff_cv_min.rejects(),
            "a floor below every measured live value costs nothing and rejects a flat presentation"
        );
        assert!(!l.response_min.rejects(), "no screen sample exists yet");
        assert!(!l.motion_max.rejects(), "statistic is untested; B9 killed the previous one");
    }

    /// The flatness floor must stay strictly under the worst live falloff ever measured
    /// over a real face box, or it starts costing false rejects on honest faces.
    #[test]
    fn the_flatness_floor_sits_below_every_measured_live_value() {
        const WORST_LIVE_FALLOFF_CV: f64 = 0.2661; // live_lit_angle_left @320
        let floor = LivenessConfig::default().falloff_cv_min.value;
        assert!(floor > 0.0, "a floor of zero rejects nothing at all");
        assert!(
            floor < WORST_LIVE_FALLOFF_CV / 2.0,
            "floor {floor} leaves less than 2x margin under the worst live capture"
        );
    }

    /// A burst that yields no pairs must not be able to satisfy a conjunctive check.
    #[test]
    fn min_pairs_is_never_zero() {
        assert!(Config::default().min_pairs >= 2);
    }

    /// The gate exists to close before `dark_ratio_max` starts lying, so it has to sit
    /// under the floor at which that check begins false-rejecting honest captures.
    ///
    /// `mean(OFF)/mean(ON) ~ F/(F+A)`, so the dark check survives only while
    /// `F < A/999`. Anchored to the weakest live emitter amplitude ever measured.
    #[test]
    fn the_gate_closes_before_the_dark_frame_check_breaks() {
        const WEAKEST_LIVE_AMPLITUDE: f64 = 48.0;
        let c = Config::default();
        let breaks_at = WEAKEST_LIVE_AMPLITUDE * c.liveness.dark_ratio_max.value
            / (1.0 - c.liveness.dark_ratio_max.value);
        assert!(
            c.ambient_floor_max <= breaks_at,
            "gate at {} sits above {breaks_at}, where dark_ratio_max starts false-rejecting",
            c.ambient_floor_max
        );
    }

    /// And it must stay clear of the noisiest honest dark capture, or it gates out the
    /// conditions the whole system is calibrated for.
    #[test]
    fn the_gate_does_not_close_on_a_dark_room() {
        const NOISIEST_DARK_FLOOR: f64 = 0.0020; // null_empty_room
        assert!(
            Config::default().ambient_floor_max > NOISIEST_DARK_FLOOR * 10.0,
            "gate leaves under 10x margin over the noisiest measured dark capture"
        );
    }

    /// Matching is k-of-n. If k ever equals n it has silently become conjunctive, which
    /// is the liveness rule applied to the wrong thing -- a blink would fail the attempt.
    #[test]
    fn matching_aggregation_is_not_conjunctive() {
        let m = MatchConfig::default();
        assert!(m.agree_k < m.agree_n, "{}-of-{} is conjunctive", m.agree_k, m.agree_n);
        assert!(m.agree_k >= 2, "a single frame must not decide identity");
    }
}
