//! Calibrated confidence gating: decide when a step's answer may skip human approval.
//!
//! Borrowed from the conformal-prediction line of work (Quasar, arXiv:2506.12202;
//! conformal risk control, Angelopoulos et al. 2022). Everything here is integer
//! arithmetic on permille values (0..=1000), so the same calibration file, alpha and
//! score always give the same decision on every machine. That keeps the decision
//! replayable and lets an auditor recompute it from the evidence bundle.
//!
//! # What is guaranteed
//!
//! The guarantee is about wrong answers, not about the approved ones. For a new case whose
//! answer is wrong, the chance that the gate waves it through without a human is at most
//! `alpha` (the "false approval rate"). It holds when the wrong calibration examples and the
//! new wrong case are exchangeable, and it is exact for any sample size (no asymptotics).
//!
//! What it does NOT say: it is not "at most `alpha` of the approved answers are wrong".
//! That share also depends on how often the step is wrong in the first place. The CLI prints
//! the calibration set's own share (`wrong_accepted / accepted`) next to the threshold so the
//! two are not confused. It also does not hold if the data drifts away from what was
//! calibrated.
//!
//! The calibration set needs enough wrong examples to certify `alpha`: at least
//! `1000 / alpha_permille - 1` of them (19 for 5%). With fewer, or with none, the threshold is
//! `NEVER` and every case goes to a human.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Highest score. Scores are permille: 0 = no confidence, 1000 = certain.
pub const MAX_SCORE: u32 = 1000;

/// Threshold meaning "never auto-approve" (no score can reach it).
pub const NEVER: u32 = MAX_SCORE + 1;

/// Calibration file format version.
pub const CALIBRATION_VERSION: u32 = 1;

/// One past case: the confidence the step reported and whether its answer was right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub score: u32,
    pub correct: bool,
}

/// A labelled calibration set, as stored in `calibration.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSet {
    pub version: u32,
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfidenceError {
    #[error("calibration file is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("calibration version {found} is not supported (expected {CALIBRATION_VERSION})")]
    UnsupportedVersion { found: u32 },
    #[error("calibration has no examples")]
    Empty,
    #[error("calibration example {index} has score {score}, must be 0..={MAX_SCORE}")]
    ScoreOutOfRange { index: usize, score: u32 },
    #[error("alpha_permille {0} is out of range, must be 1..=999")]
    AlphaOutOfRange(u32),
}

impl CalibrationSet {
    /// Parse and validate a calibration file.
    pub fn from_json(json: &str) -> Result<Self, ConfidenceError> {
        let set: CalibrationSet =
            serde_json::from_str(json).map_err(|e| ConfidenceError::InvalidJson(e.to_string()))?;
        if set.version != CALIBRATION_VERSION {
            return Err(ConfidenceError::UnsupportedVersion { found: set.version });
        }
        if set.examples.is_empty() {
            return Err(ConfidenceError::Empty);
        }
        if let Some((index, e)) = set
            .examples
            .iter()
            .enumerate()
            .find(|(_, e)| e.score > MAX_SCORE)
        {
            return Err(ConfidenceError::ScoreOutOfRange {
                index,
                score: e.score,
            });
        }
        Ok(set)
    }
}

/// SHA-256 (lowercase hex) of the exact calibration bytes.
pub fn calibration_sha256(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Smallest score threshold that keeps the false approval rate at or below `alpha`.
///
/// Auto-approve a case when `score >= threshold`. For a candidate threshold `t`, the
/// conformal p-value of a wrong case scoring `t` is
/// `(1 + wrong_at_or_above_t) / (1 + wrong_total)`, counted over the wrong calibration
/// examples only. It falls as `t` rises, so the first candidate with `p <= alpha` is the one
/// that approves the most. Written without division:
/// `(wrong_at_or_above + 1) * 1000 <= alpha_permille * (wrong_total + 1)`.
///
/// Returns [`NEVER`] when no threshold passes.
pub fn risk_threshold(set: &CalibrationSet, alpha_permille: u32) -> Result<u32, ConfidenceError> {
    if !(1..=999).contains(&alpha_permille) {
        return Err(ConfidenceError::AlphaOutOfRange(alpha_permille));
    }
    let wrong_total = set.examples.iter().filter(|e| !e.correct).count() as u64;
    let budget = u64::from(alpha_permille) * (wrong_total + 1);

    // Candidates: accept everything (0) and each distinct example score.
    let mut candidates: Vec<u32> = set.examples.iter().map(|e| e.score).collect();
    candidates.push(0);
    candidates.sort_unstable();
    candidates.dedup();

    for lambda in candidates {
        let wrong_above = set
            .examples
            .iter()
            .filter(|e| e.score >= lambda && !e.correct)
            .count() as u64;
        if (wrong_above + 1) * u64::from(MAX_SCORE) <= budget {
            return Ok(lambda);
        }
    }
    Ok(NEVER)
}

/// What the calibration set itself looks like at a threshold: how many examples would be
/// auto-approved and how many of those were wrong. Informational, not a guarantee.
pub fn calibration_share(set: &CalibrationSet, threshold: u32) -> (usize, usize) {
    let accepted = set.examples.iter().filter(|e| e.score >= threshold);
    let (mut n, mut wrong) = (0, 0);
    for e in accepted {
        n += 1;
        if !e.correct {
            wrong += 1;
        }
    }
    (n, wrong)
}

/// What the gate does with one score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Score reached the threshold: skip the human.
    AutoApproved,
    /// Score is below the threshold, missing, or invalid: a human decides.
    Escalated,
}

/// Decide for one case. A missing or out-of-range score always escalates.
pub fn decide(threshold: u32, score: Option<u32>) -> Decision {
    match score {
        Some(s) if s <= MAX_SCORE && s >= threshold => Decision::AutoApproved,
        _ => Decision::Escalated,
    }
}

/// One gate evaluation, as written to `confidence_gates.json` in the evidence bundle.
/// Holds everything an auditor needs to recompute the decision from the embedded
/// calibration set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRecord {
    pub step_id: String,
    pub source_step: String,
    pub alpha_permille: u32,
    pub calibration_sha256: String,
    pub calibration_examples: usize,
    pub threshold_permille: u32,
    pub score_permille: Option<u32>,
    pub decision: Decision,
}

impl GateRecord {
    /// Recompute the threshold and decision from a calibration set and compare to
    /// what was recorded. `Err` names the first mismatch.
    pub fn verify(&self, set: &CalibrationSet, raw: &[u8]) -> Result<(), String> {
        if calibration_sha256(raw) != self.calibration_sha256 {
            return Err(format!(
                "gate '{}': calibration hash does not match the embedded file",
                self.step_id
            ));
        }
        if set.examples.len() != self.calibration_examples {
            return Err(format!(
                "gate '{}': recorded {} examples, file has {}",
                self.step_id,
                self.calibration_examples,
                set.examples.len()
            ));
        }
        let threshold = risk_threshold(set, self.alpha_permille)
            .map_err(|e| format!("gate '{}': {e}", self.step_id))?;
        if threshold != self.threshold_permille {
            return Err(format!(
                "gate '{}': recorded threshold {} but the calibration gives {}",
                self.step_id, self.threshold_permille, threshold
            ));
        }
        let decision = decide(threshold, self.score_permille);
        if decision != self.decision {
            return Err(format!(
                "gate '{}': recorded decision {:?} but the score gives {:?}",
                self.step_id, self.decision, decision
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(u32, bool)]) -> CalibrationSet {
        CalibrationSet {
            version: CALIBRATION_VERSION,
            examples: pairs
                .iter()
                .map(|&(score, correct)| Example { score, correct })
                .collect(),
        }
    }

    /// 100 examples: scores 0, 10, .. 990; the answer is wrong for every score below 500.
    fn clean_split() -> CalibrationSet {
        let pairs: Vec<(u32, bool)> = (0..100u32).map(|i| (i * 10, i * 10 >= 500)).collect();
        set(&pairs)
    }

    #[test]
    fn threshold_is_lowest_score_that_keeps_false_approval_under_alpha() {
        // 50 wrong examples (scores 0..=490). alpha 5%: budget = 50 * 51 = 2550, so
        // (wrong_at_or_above + 1) * 1000 <= 2550 allows at most 1 wrong at or above.
        // Score 490 has exactly 1 wrong at or above (itself); 480 has 2.
        assert_eq!(risk_threshold(&clean_split(), 50).unwrap(), 490);
    }

    #[test]
    fn higher_alpha_never_raises_the_threshold() {
        let s = clean_split();
        let mut prev = NEVER;
        for alpha in [10, 50, 100, 200, 500, 900] {
            let t = risk_threshold(&s, alpha).unwrap();
            assert!(t <= prev, "alpha {alpha} gave {t} > {prev}");
            prev = t;
        }
    }

    #[test]
    fn too_few_wrong_examples_to_certify_means_never() {
        // alpha 5% needs at least 19 wrong examples. Here: 10 wrong, 90 right.
        let mut pairs: Vec<(u32, bool)> = (0..10u32).map(|i| (i * 10, false)).collect();
        pairs.extend((0..90u32).map(|i| (500 + i * 5, true)));
        assert_eq!(risk_threshold(&set(&pairs), 50).unwrap(), NEVER);
        // 19 wrong is enough.
        let mut pairs: Vec<(u32, bool)> = (0..19u32).map(|i| (i * 10, false)).collect();
        pairs.extend((0..90u32).map(|i| (500 + i * 5, true)));
        assert_ne!(risk_threshold(&set(&pairs), 50).unwrap(), NEVER);
    }

    #[test]
    fn calibration_with_no_wrong_example_never_approves() {
        // Nothing to calibrate against: refuse rather than approve everything.
        let pairs: Vec<(u32, bool)> = (0..100u32).map(|i| (i * 10, true)).collect();
        assert_eq!(risk_threshold(&set(&pairs), 50).unwrap(), NEVER);
    }

    #[test]
    fn all_wrong_calibration_only_approves_the_top_slice() {
        // 100 wrong examples, alpha 5%: at most 4 wrong at or above -> threshold 960.
        let pairs: Vec<(u32, bool)> = (0..100u32).map(|i| (i * 10, false)).collect();
        assert_eq!(risk_threshold(&set(&pairs), 50).unwrap(), 960);
    }

    #[test]
    fn share_reports_what_the_calibration_set_looks_like_at_a_threshold() {
        let s = clean_split();
        // At 490: scores 490..=990 -> 51 examples, 1 wrong.
        assert_eq!(calibration_share(&s, 490), (51, 1));
        assert_eq!(calibration_share(&s, NEVER), (0, 0));
    }

    #[test]
    fn decide_boundary_and_failure_modes() {
        assert_eq!(decide(460, Some(460)), Decision::AutoApproved);
        assert_eq!(decide(460, Some(459)), Decision::Escalated);
        assert_eq!(decide(NEVER, Some(1000)), Decision::Escalated);
        assert_eq!(decide(0, None), Decision::Escalated);
        assert_eq!(decide(0, Some(1001)), Decision::Escalated);
    }

    #[test]
    fn rejects_bad_alpha_and_bad_files() {
        let s = clean_split();
        assert_eq!(
            risk_threshold(&s, 0),
            Err(ConfidenceError::AlphaOutOfRange(0))
        );
        assert_eq!(
            risk_threshold(&s, 1000),
            Err(ConfidenceError::AlphaOutOfRange(1000))
        );
        assert!(matches!(
            CalibrationSet::from_json("nope"),
            Err(ConfidenceError::InvalidJson(_))
        ));
        assert_eq!(
            CalibrationSet::from_json(r#"{"version":2,"examples":[{"score":1,"correct":true}]}"#),
            Err(ConfidenceError::UnsupportedVersion { found: 2 })
        );
        assert_eq!(
            CalibrationSet::from_json(r#"{"version":1,"examples":[]}"#),
            Err(ConfidenceError::Empty)
        );
        assert_eq!(
            CalibrationSet::from_json(
                r#"{"version":1,"examples":[{"score":1001,"correct":true}]}"#
            ),
            Err(ConfidenceError::ScoreOutOfRange {
                index: 0,
                score: 1001
            })
        );
    }

    #[test]
    fn example_order_does_not_change_the_threshold() {
        let a = clean_split();
        let mut b = a.clone();
        b.examples.reverse();
        assert_eq!(
            risk_threshold(&a, 50).unwrap(),
            risk_threshold(&b, 50).unwrap()
        );
    }

    /// Empirical check of the guarantee: calibrate on one stream, test on a fresh one.
    /// Deterministic LCG, so this is not flaky. The guarantee holds in expectation over
    /// calibration draws, so one draw can land slightly above alpha; the slack below is that
    /// sampling noise, not a weaker promise.
    #[test]
    fn wrong_answers_are_waved_through_at_most_alpha_of_the_time() {
        let mut x: u64 = 12345;
        let mut next = move || {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u32
        };
        // Confidence s in 0..=1000; the answer is right with probability s/1000.
        let mut gen = |n: usize| -> Vec<Example> {
            (0..n)
                .map(|_| {
                    let score = next() % 1001;
                    let correct = next() % 1000 < score;
                    Example { score, correct }
                })
                .collect()
        };
        for alpha in [50u32, 100, 200] {
            let calib = CalibrationSet {
                version: CALIBRATION_VERSION,
                examples: gen(20000),
            };
            let threshold = risk_threshold(&calib, alpha).unwrap();
            let test = gen(50000);
            let wrong = test.iter().filter(|e| !e.correct).count();
            let waved = test
                .iter()
                .filter(|e| {
                    !e.correct && decide(threshold, Some(e.score)) == Decision::AutoApproved
                })
                .count();
            let rate_permille = waved * 1000 / wrong;
            assert!(
                rate_permille <= alpha as usize + 5,
                "alpha {alpha}: {rate_permille} permille of wrong answers were auto-approved"
            );
        }
    }

    #[test]
    fn gate_record_verifies_and_detects_tampering() {
        let raw = serde_json::to_vec(&clean_split()).unwrap();
        let s = CalibrationSet::from_json(std::str::from_utf8(&raw).unwrap()).unwrap();
        let t = risk_threshold(&s, 50).unwrap();
        let rec = GateRecord {
            step_id: "gate".into(),
            source_step: "classify".into(),
            alpha_permille: 50,
            calibration_sha256: calibration_sha256(&raw),
            calibration_examples: s.examples.len(),
            threshold_permille: t,
            score_permille: Some(900),
            decision: Decision::AutoApproved,
        };
        assert_eq!(rec.verify(&s, &raw), Ok(()));

        let mut forged = rec.clone();
        forged.decision = Decision::Escalated;
        assert!(forged.verify(&s, &raw).unwrap_err().contains("decision"));

        let mut forged = rec.clone();
        forged.threshold_permille = 0;
        assert!(forged.verify(&s, &raw).unwrap_err().contains("threshold"));

        let mut other = raw.clone();
        other.push(b' ');
        assert!(rec.verify(&s, &other).unwrap_err().contains("hash"));
    }
}
