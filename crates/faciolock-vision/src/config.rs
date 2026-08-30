#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enforcement {
    Enforced,
    Advisory,
}

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
    pub fn rejects(&self) -> bool {
        self.enforcement == Enforcement::Enforced
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    IrStrobe,
    IrConstant,
    Rgb,
}

#[derive(Clone, Copy, Debug)]
pub struct LivenessConfig {
    pub dark_ratio_max: Threshold,

    pub response_min: Threshold,

    pub falloff_cv_min: Threshold,

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
    pub input_size: usize,
    pub score_min: f32,
    pub nms_iou_max: f32,
}

impl Default for DetectConfig {
    fn default() -> DetectConfig {
        DetectConfig { input_size: 320, score_min: 0.3, nms_iou_max: 0.3 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MatchConfig {
    pub cosine_min: f64,
    pub agree_k: usize,
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
    pub agc_settle_frames: usize,
    pub min_pairs: usize,
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

    #[test]
    fn the_flatness_floor_sits_below_every_measured_live_value() {
        const WORST_LIVE_FALLOFF_CV: f64 = 0.2661;
        let floor = LivenessConfig::default().falloff_cv_min.value;
        assert!(floor > 0.0, "a floor of zero rejects nothing at all");
        assert!(
            floor < WORST_LIVE_FALLOFF_CV / 2.0,
            "floor {floor} leaves less than 2x margin under the worst live capture"
        );
    }

    #[test]
    fn min_pairs_is_never_zero() {
        assert!(Config::default().min_pairs >= 2);
    }

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

    #[test]
    fn the_gate_does_not_close_on_a_dark_room() {
        const NOISIEST_DARK_FLOOR: f64 = 0.0020;
        assert!(
            Config::default().ambient_floor_max > NOISIEST_DARK_FLOOR * 10.0,
            "gate leaves under 10x margin over the noisiest measured dark capture"
        );
    }

    #[test]
    fn matching_aggregation_is_not_conjunctive() {
        let m = MatchConfig::default();
        assert!(m.agree_k < m.agree_n, "{}-of-{} is conjunctive", m.agree_k, m.agree_n);
        assert!(m.agree_k >= 2, "a single frame must not decide identity");
    }
}
