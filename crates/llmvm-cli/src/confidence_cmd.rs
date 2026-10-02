//! `boruna confidence threshold` — inspect a calibration file before wiring it to a gate.

use std::path::Path;

use boruna_orchestrator::confidence::{
    calibration_sha256, calibration_share, decide, risk_threshold, CalibrationSet, Decision, NEVER,
};

/// Print the threshold a calibration file gives for `alpha_permille`, and optionally the
/// decision for one score. Returns the process exit code.
pub fn run_threshold(
    file: &Path,
    alpha_permille: u32,
    score: Option<u32>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let text = std::str::from_utf8(&raw).map_err(|e| format!("calibration is not UTF-8: {e}"))?;
    let set = CalibrationSet::from_json(text)?;
    let threshold = risk_threshold(&set, alpha_permille)?;
    let wrong = set.examples.iter().filter(|e| !e.correct).count();
    let (accepted, wrong_accepted) = calibration_share(&set, threshold);
    let needed_wrong = (1000 / alpha_permille).saturating_sub(1) as usize;
    let decision = score.map(|s| decide(threshold, Some(s)));
    let never = threshold == NEVER;

    if json {
        let payload = serde_json::json!({
            "calibration_sha256": calibration_sha256(&raw),
            "examples": set.examples.len(),
            "wrong_examples": wrong,
            "alpha_permille": alpha_permille,
            "threshold_permille": if never { serde_json::Value::Null } else { threshold.into() },
            "never_auto_approves": never,
            "wrong_examples_needed": needed_wrong,
            "calibration_accepted": accepted,
            "calibration_wrong_accepted": wrong_accepted,
            "score_permille": score,
            "decision": decision,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("file         {}", file.display());
    println!("sha256       {}", calibration_sha256(&raw));
    println!("examples     {} ({} wrong)", set.examples.len(), wrong);
    println!("alpha        {alpha_permille} permille");
    if never {
        println!("threshold    never auto-approves");
        if wrong < needed_wrong {
            println!(
                "reason       {wrong} wrong examples; at least {needed_wrong} are needed to \
                 certify alpha {alpha_permille} permille"
            );
        } else {
            println!("reason       no score keeps the false approval rate within alpha");
        }
    } else {
        println!("threshold    {threshold} (auto-approve at score >= {threshold})");
        println!(
            "in the set   would approve {accepted} of {} examples; {wrong_accepted} of those were wrong",
            set.examples.len()
        );
        println!(
            "guarantee    a wrong answer is auto-approved with probability <= {alpha_permille} permille"
        );
    }
    if let (Some(s), Some(d)) = (score, decision) {
        let word = match d {
            Decision::AutoApproved => "auto-approved",
            Decision::Escalated => "escalated to a human",
        };
        println!("score {s}      {word}");
    }
    Ok(())
}
