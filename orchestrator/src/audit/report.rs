//! Compliance evidence-mapping reports (`boruna evidence report`).
//!
//! Turns the machine-facts already inside an evidence bundle (run id,
//! hash-chained audit log, policy/workflow hashes, env fingerprint,
//! signature) into a HUMAN-READABLE mapping from each present artifact
//! to the specific regulatory obligation it helps satisfy.
//!
//! This is deliberately NOT a certificate of compliance. It is a
//! *technical mapping*: "here is the record, and here is the obligation
//! text it speaks to." Obligations the bundle does NOT cover are flagged
//! loudly (e.g. retention metadata, which the bundle format does not
//! carry). The report ALWAYS runs `verify_bundle` first — a mapping over
//! a tampered or unverifiable bundle is worse than useless, so the
//! verification verdict is stamped at the top and any failure is shown
//! prominently.

use std::path::Path;

use crate::audit::evidence::BundleManifest;
use crate::audit::log::{AuditEvent, AuditLog};
use crate::audit::verify::verify_bundle;

/// Regulatory framework the report maps evidence against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplianceFramework {
    /// EU AI Act (Regulation (EU) 2024/1689) record-keeping obligations.
    EuAiAct,
    /// NIST AI Risk Management Framework 1.0.
    Nist,
    /// ISO/IEC 42001:2023 AI management system.
    Iso42001,
}

impl ComplianceFramework {
    /// Parse the CLI `--framework` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "eu-ai-act" | "eu_ai_act" | "euaiact" => Ok(ComplianceFramework::EuAiAct),
            "nist" | "nist-ai-rmf" => Ok(ComplianceFramework::Nist),
            "iso42001" | "iso-42001" | "iso" => Ok(ComplianceFramework::Iso42001),
            other => Err(format!(
                "unknown framework {other:?} (expected: eu-ai-act | nist | iso42001)"
            )),
        }
    }

    fn title(self) -> &'static str {
        match self {
            ComplianceFramework::EuAiAct => "EU AI Act (Regulation (EU) 2024/1689)",
            ComplianceFramework::Nist => "NIST AI Risk Management Framework 1.0",
            ComplianceFramework::Iso42001 => "ISO/IEC 42001:2023",
        }
    }
}

/// Output rendering for a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Markdown,
    Html,
}

impl ReportFormat {
    /// Parse the CLI `--format` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Ok(ReportFormat::Markdown),
            "html" => Ok(ReportFormat::Html),
            other => Err(format!("unknown format {other:?} (expected: md | html)")),
        }
    }
}

/// How well the recorded evidence speaks to an obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coverage {
    /// The bundle provides a record that directly satisfies this obligation.
    Provided,
    /// The bundle covers part of the obligation; the rest is out of scope
    /// for what a sealed run-artifact can attest.
    Partial,
    /// The bundle carries no evidence for this obligation.
    NotCovered,
    /// The obligation does not apply to this particular run.
    NotApplicable,
}

impl Coverage {
    fn label(self) -> &'static str {
        match self {
            Coverage::Provided => "EVIDENCE PROVIDED",
            Coverage::Partial => "PARTIAL",
            Coverage::NotCovered => "NOT COVERED",
            Coverage::NotApplicable => "NOT APPLICABLE",
        }
    }
}

/// One obligation row: the regulatory reference, what the bundle provides
/// for it (with the actual run's hashes), and — critically — any gap.
struct Obligation {
    reference: String,
    title: String,
    coverage: Coverage,
    provides: String,
    gap: Option<String>,
}

/// Facts distilled from a bundle's manifest + audit log, used to build the
/// obligation mapping. Absent components are recorded as `false`/`None` so
/// the mapping can flag them honestly.
struct BundleFacts {
    run_id: String,
    workflow_name: String,
    workflow_hash: String,
    policy_hash: String,
    audit_log_hash: String,
    bundle_hash: String,
    started_at: String,
    completed_at: String,
    env_summary: String,
    signed: bool,
    signer_pubkey: Option<String>,
    encrypted: bool,
    has_workflow: bool,
    has_policy: bool,
    has_outputs: bool,
    has_intents: bool,
    has_model_invocations: bool,
    /// `None` when the audit log could not be parsed (e.g. encrypted and
    /// undecryptable in report context).
    audit_event_count: Option<usize>,
    /// Human-readable approval-gate (human-oversight) records, if any.
    approvals: Vec<String>,
}

impl BundleFacts {
    fn from_bundle(bundle_dir: &Path, manifest: &BundleManifest) -> Self {
        let (audit_event_count, approvals) = read_audit_facts(bundle_dir);
        let env = &manifest.env_fingerprint;
        BundleFacts {
            run_id: manifest.run_id.clone(),
            workflow_name: manifest.workflow_name.clone(),
            workflow_hash: manifest.workflow_hash.clone(),
            policy_hash: manifest.policy_hash.clone(),
            audit_log_hash: manifest.audit_log_hash.clone(),
            bundle_hash: manifest.bundle_hash.clone(),
            started_at: manifest.started_at.clone(),
            completed_at: manifest.completed_at.clone(),
            env_summary: format!(
                "{} on {}/{}, boruna {}",
                env.rust_version, env.os, env.arch, env.boruna_version
            ),
            signed: manifest.signature.is_some(),
            signer_pubkey: manifest.signature.as_ref().map(|s| s.public_key.clone()),
            encrypted: manifest.encryption.is_some(),
            has_workflow: bundle_dir.join("workflow.json").exists(),
            has_policy: bundle_dir.join("policy.json").exists(),
            has_outputs: bundle_dir.join("outputs").is_dir(),
            has_intents: bundle_dir.join("intents.json").exists(),
            has_model_invocations: bundle_dir.join("model_invoking_steps.json").exists(),
            audit_event_count,
            approvals,
        }
    }
}

/// Read the audit log for the event count and any approval-gate records.
/// Degrades gracefully: on any read/parse failure returns `(None, [])`.
fn read_audit_facts(bundle_dir: &Path) -> (Option<usize>, Vec<String>) {
    let raw = match std::fs::read_to_string(bundle_dir.join("audit_log.json")) {
        Ok(s) => s,
        Err(_) => return (None, Vec::new()),
    };
    let log = match AuditLog::from_json(&raw) {
        Ok(l) => l,
        Err(_) => return (None, Vec::new()),
    };
    let mut approvals = Vec::new();
    for entry in log.entries() {
        match &entry.event {
            AuditEvent::ApprovalRequested { step_id, role } => {
                approvals.push(format!(
                    "step `{step_id}`: approval requested from role `{role}`"
                ));
            }
            AuditEvent::ApprovalGranted { step_id, approver } => {
                approvals.push(format!(
                    "step `{step_id}`: approval GRANTED by `{approver}`"
                ));
            }
            AuditEvent::ApprovalDenied { step_id, reason } => {
                approvals.push(format!("step `{step_id}`: approval DENIED ({reason})"));
            }
            _ => {}
        }
    }
    (Some(log.entries().len()), approvals)
}

/// Generate a compliance evidence-mapping report for a bundle.
///
/// The bundle is VERIFIED first; the verdict (and any errors) is stamped
/// at the top of the report. A tampered/unverifiable bundle still produces
/// a report — one that says so loudly — so an auditor is never handed a
/// clean-looking mapping over broken evidence.
///
/// Returns `Err` only when the manifest itself cannot be read/parsed (there
/// is then nothing to map); a bundle that merely fails verification returns
/// `Ok` with the failure surfaced in the report body.
pub fn generate_report(
    bundle_dir: &Path,
    framework: ComplianceFramework,
    format: ReportFormat,
) -> Result<String, String> {
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read manifest.json: {e}"))?;
    let manifest: BundleManifest =
        serde_json::from_str(&manifest_json).map_err(|e| format!("invalid manifest.json: {e}"))?;

    let verdict = verify_bundle(bundle_dir);
    let facts = BundleFacts::from_bundle(bundle_dir, &manifest);
    let obligations = match framework {
        ComplianceFramework::EuAiAct => eu_ai_act_obligations(&facts),
        ComplianceFramework::Nist => nist_obligations(&facts),
        ComplianceFramework::Iso42001 => iso42001_obligations(&facts),
    };

    Ok(match format {
        ReportFormat::Markdown => render_markdown(framework, &facts, &verdict, &obligations),
        ReportFormat::Html => render_html(framework, &facts, &verdict, &obligations),
    })
}

// ---- Obligation catalogues ------------------------------------------------

fn eu_ai_act_obligations(f: &BundleFacts) -> Vec<Obligation> {
    let mut out = Vec::new();

    // Art. 12(2)(a-c) — automatic recording of events (logging).
    let events = f
        .audit_event_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    out.push(Obligation {
        reference: "Art. 12(2)(a–c)".to_string(),
        title: "Automatic recording of events (logging) over the system's lifetime".to_string(),
        coverage: if f.audit_event_count.is_some() {
            Coverage::Provided
        } else {
            Coverage::NotCovered
        },
        provides: format!(
            "`audit_log.json` — a hash-chained (SHA-256) event log with {events} entries, \
             head hash `audit_log_hash = {}`. Each entry chains the previous entry's hash, so \
             any insertion, deletion, or edit is detectable. This is the automatically generated \
             record of events over the run's lifetime.",
            f.audit_log_hash
        ),
        gap: if f.audit_event_count.is_none() {
            Some(
                "The audit log could not be read (bundle may be encrypted); the event record \
                 cannot be summarised without the decryption key."
                    .to_string(),
            )
        } else {
            None
        },
    });

    // Art. 12(3) — Annex III logging content.
    let period_ok = !f.started_at.is_empty() && !f.completed_at.is_empty();
    let persons_ok = !f.approvals.is_empty();
    let all_ok = period_ok && f.has_outputs && persons_ok;
    let mut sub = Vec::new();
    sub.push(format!(
        "- period of each use: recorded as `started_at = {}` .. `completed_at = {}` [{}]",
        f.started_at,
        f.completed_at,
        if period_ok { "PROVIDED" } else { "MISSING" }
    ));
    sub.push(format!(
        "- input data / records checked against: per-step outputs under `outputs/` [{}]",
        if f.has_outputs {
            "PROVIDED"
        } else {
            "NOT PRESENT"
        }
    ));
    sub.push(format!(
        "- natural persons verifying results: approval-gate records [{}]",
        if persons_ok {
            "PROVIDED"
        } else {
            "NONE RECORDED"
        }
    ));
    out.push(Obligation {
        reference: "Art. 12(3)".to_string(),
        title: "Logging content for Annex III high-risk systems".to_string(),
        coverage: if all_ok {
            Coverage::Provided
        } else {
            Coverage::Partial
        },
        provides: sub.join("\n"),
        gap: if all_ok {
            None
        } else {
            Some(
                "Not every Annex III logging item is present in this run. Items marked MISSING / \
                 NONE RECORDED are either not applicable to a fully-automated run (no human \
                 verifier) or must be supplied by the deploying system — the bundle attests only \
                 what the run actually recorded."
                    .to_string(),
            )
        },
    });

    // Art. 19 + Art. 26(6) — retention >= 6 months. The bundle format
    // carries NO retention metadata, so this is always flagged.
    out.push(Obligation {
        reference: "Art. 19 & Art. 26(6)".to_string(),
        title: "Automatic logs kept / retained for at least 6 months".to_string(),
        coverage: Coverage::NotCovered,
        provides: "The bundle is a sealed, tamper-evident snapshot but declares no retention \
                   period or lifecycle policy."
            .to_string(),
        gap: Some(
            "retention policy: NOT DECLARED — Art. 19 (provider) and Art. 26(6) (deployer) \
             require the automatically generated logs to be retained for a period appropriate to \
             the intended purpose, and at least 6 months unless other law provides otherwise. \
             Retention must be enforced by the operator's storage/lifecycle controls; it is NOT \
             attested by this evidence bundle."
                .to_string(),
        ),
    });

    // Art. 14 — human oversight (approval-gate records).
    if f.approvals.is_empty() {
        out.push(Obligation {
            reference: "Art. 14".to_string(),
            title: "Human oversight".to_string(),
            coverage: Coverage::NotApplicable,
            provides: "No approval-gate (human-oversight) events were recorded in this run's \
                       audit log."
                .to_string(),
            gap: Some(
                "If this system is subject to Art. 14 human-oversight requirements, the workflow \
                 did not record a human approval gate. A fully-automated run cannot evidence \
                 human oversight — add an approval step to capture it."
                    .to_string(),
            ),
        });
    } else {
        out.push(Obligation {
            reference: "Art. 14".to_string(),
            title: "Human oversight".to_string(),
            coverage: Coverage::Provided,
            provides: format!(
                "Approval-gate records in the audit log evidence human oversight:\n{}",
                f.approvals
                    .iter()
                    .map(|a| format!("- {a}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            gap: None,
        });
    }

    out
}

fn nist_obligations(f: &BundleFacts) -> Vec<Obligation> {
    let mut out = Vec::new();

    let signer = f
        .signer_pubkey
        .as_deref()
        .map(|k| format!(", ed25519-signed by `{k}`"))
        .unwrap_or_default();
    out.push(Obligation {
        reference: "MEASURE 2.x (2.8 / 2.9 / 2.11)".to_string(),
        title: "Traceability, provenance, and the ability to inspect/audit AI system behaviour"
            .to_string(),
        coverage: Coverage::Provided,
        provides: format!(
            "The bundle is a replayable provenance record: `bundle_hash = {}`{}, workflow hash \
             `{}`, audit head `{}`, and an environment fingerprint ({}). Per-file SHA-256 \
             checksums plus the hash-chained log let a third party re-inspect exactly what ran \
             and confirm nothing changed.",
            f.bundle_hash, signer, f.workflow_hash, f.audit_log_hash, f.env_summary
        ),
        gap: None,
    });

    out.push(Obligation {
        reference: "MANAGE 2.x".to_string(),
        title: "Documented policy / capability controls governing the system".to_string(),
        coverage: if f.has_policy {
            Coverage::Provided
        } else {
            Coverage::NotCovered
        },
        provides: if f.has_policy {
            format!(
                "`policy.json` captures the capability policy in force during the run \
                 (`policy_hash = {}`); capability decisions are recorded in the audit log.",
                f.policy_hash
            )
        } else {
            "No `policy.json` component is present in this bundle.".to_string()
        },
        gap: if f.has_policy {
            None
        } else {
            Some(
                "policy record: NOT PRESENT — MANAGE expects the governing controls to be \
                 documented; this bundle carries no policy snapshot."
                    .to_string(),
            )
        },
    });

    out
}

fn iso42001_obligations(f: &BundleFacts) -> Vec<Obligation> {
    let mut out = Vec::new();

    let integrity = if f.signed {
        "sealed with a bundle hash AND an ed25519 signature"
    } else {
        "sealed with a bundle hash (unsigned)"
    };
    out.push(Obligation {
        reference: "Clause 7.5 / 8.1".to_string(),
        title: "Control of documented information & operational records".to_string(),
        coverage: if f.signed {
            Coverage::Provided
        } else {
            Coverage::Partial
        },
        provides: format!(
            "The evidence bundle for run `{}` (workflow `{}`) is a controlled record: {}, \
             `bundle_hash = {}`. Its contents are protected against unintended alteration by \
             per-file checksums and the hash-chained audit log.{}",
            f.run_id,
            f.workflow_name,
            integrity,
            f.bundle_hash,
            if f.encrypted {
                " Contents are additionally encrypted at rest."
            } else {
                ""
            }
        ),
        gap: if f.signed {
            None
        } else {
            Some(
                "The record is tamper-EVIDENT (hash-chained) but UNSIGNED — integrity rests on \
                 an out-of-band anchor of `bundle_hash`. Sign the bundle (ed25519) to root record \
                 integrity in an operator key rather than external anchoring."
                    .to_string(),
            )
        },
    });

    out
}

// ---- Rendering ------------------------------------------------------------

const DISCLAIMER: &str = "This is a TECHNICAL EVIDENCE-MAPPING report, NOT a certificate of \
compliance and NOT legal attestation. It maps artifacts present in a Boruna evidence bundle to the \
regulatory obligations they help evidence; it does not assess whether the deploying organisation \
meets those obligations. Determinism guarantees reproducibility GIVEN the recorded effects \
(captured capability results) — it does NOT prove reproducibility of any underlying AI model's \
outputs. Obligations flagged NOT COVERED / NOT DECLARED require controls outside this bundle. \
Consult qualified counsel for a compliance determination.";

fn render_markdown(
    framework: ComplianceFramework,
    f: &BundleFacts,
    verdict: &crate::audit::verify::VerifyResult,
    obligations: &[Obligation],
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# Compliance Evidence Mapping — {}\n\n",
        framework.title()
    ));
    s.push_str(&format!("- **Run ID:** `{}`\n", f.run_id));
    s.push_str(&format!("- **Workflow:** `{}`\n", f.workflow_name));
    s.push_str(&format!("- **Bundle hash:** `{}`\n", f.bundle_hash));
    s.push_str(&format!("- **Audit log hash:** `{}`\n", f.audit_log_hash));
    s.push_str(&format!(
        "- **Run window:** `{}` .. `{}`\n",
        f.started_at, f.completed_at
    ));
    s.push_str(&format!("- **Environment:** {}\n", f.env_summary));
    s.push_str(&format!(
        "- **Signature:** {}\n",
        match &f.signer_pubkey {
            Some(k) => format!("ed25519 `{k}`"),
            None => "unsigned".to_string(),
        }
    ));
    s.push('\n');

    // Verification banner — loud on failure.
    if verdict.valid {
        s.push_str(
            "## Verification: PASSED\n\nThe bundle passed integrity verification \
                    (`verify_bundle`): checksums, hash chain, and required files are intact.\n\n",
        );
    } else {
        s.push_str("## Verification: FAILED\n\n");
        s.push_str(
            "> WARNING: This bundle did NOT pass integrity verification. The mapping below is \
             over EVIDENCE THAT CANNOT BE TRUSTED. Do not rely on it until the errors are \
             resolved.\n\n",
        );
        for e in &verdict.errors {
            s.push_str(&format!("- `{e}`\n"));
        }
        s.push('\n');
    }

    s.push_str("## Disclaimer\n\n");
    s.push_str(DISCLAIMER);
    s.push_str("\n\n");

    s.push_str("## Obligation mapping\n\n");
    for o in obligations {
        s.push_str(&format!(
            "### {} — {}\n\n**Status:** {}\n\n**Evidence provided:**\n\n{}\n\n",
            o.reference,
            o.title,
            o.coverage.label(),
            o.provides
        ));
        if let Some(gap) = &o.gap {
            s.push_str(&format!("**Gap / not covered:** {gap}\n\n"));
        }
    }

    s.push_str("---\n\n");
    s.push_str(&format!(
        "_Generated by `boruna evidence report` over bundle `{}`. Components observed: \
         workflow={}, policy={}, outputs={}, intents={}, model_invoking_steps={}._\n",
        f.run_id,
        f.has_workflow,
        f.has_policy,
        f.has_outputs,
        f.has_intents,
        f.has_model_invocations
    ));
    s
}

fn render_html(
    framework: ComplianceFramework,
    f: &BundleFacts,
    verdict: &crate::audit::verify::VerifyResult,
    obligations: &[Obligation],
) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>Compliance Evidence Mapping &mdash; {}</h1>\n",
        esc(framework.title())
    ));
    body.push_str("<ul class=\"meta\">\n");
    body.push_str(&format!(
        "<li><strong>Run ID:</strong> <code>{}</code></li>\n",
        esc(&f.run_id)
    ));
    body.push_str(&format!(
        "<li><strong>Workflow:</strong> <code>{}</code></li>\n",
        esc(&f.workflow_name)
    ));
    body.push_str(&format!(
        "<li><strong>Bundle hash:</strong> <code>{}</code></li>\n",
        esc(&f.bundle_hash)
    ));
    body.push_str(&format!(
        "<li><strong>Audit log hash:</strong> <code>{}</code></li>\n",
        esc(&f.audit_log_hash)
    ));
    body.push_str(&format!(
        "<li><strong>Run window:</strong> <code>{}</code> .. <code>{}</code></li>\n",
        esc(&f.started_at),
        esc(&f.completed_at)
    ));
    body.push_str(&format!(
        "<li><strong>Environment:</strong> {}</li>\n",
        esc(&f.env_summary)
    ));
    let sig = match &f.signer_pubkey {
        Some(k) => format!("ed25519 <code>{}</code>", esc(k)),
        None => "unsigned".to_string(),
    };
    body.push_str(&format!("<li><strong>Signature:</strong> {sig}</li>\n"));
    body.push_str("</ul>\n");

    if verdict.valid {
        body.push_str(
            "<div class=\"verify pass\"><h2>Verification: PASSED</h2><p>The bundle passed \
             integrity verification (checksums, hash chain, required files intact).</p></div>\n",
        );
    } else {
        body.push_str(
            "<div class=\"verify fail\"><h2>Verification: FAILED</h2><p><strong>WARNING: \
                       This bundle did NOT pass integrity verification. The mapping below is over \
                       evidence that cannot be trusted.</strong></p><ul>\n",
        );
        for e in &verdict.errors {
            body.push_str(&format!("<li><code>{}</code></li>\n", esc(e)));
        }
        body.push_str("</ul></div>\n");
    }

    body.push_str(&format!(
        "<div class=\"disclaimer\"><h2>Disclaimer</h2><p>{}</p></div>\n",
        esc(DISCLAIMER)
    ));

    body.push_str("<h2>Obligation mapping</h2>\n");
    for o in obligations {
        let cls = match o.coverage {
            Coverage::Provided => "provided",
            Coverage::Partial => "partial",
            Coverage::NotCovered => "notcovered",
            Coverage::NotApplicable => "na",
        };
        body.push_str(&format!(
            "<section class=\"ob {}\">\n<h3>{} &mdash; {}</h3>\n\
             <p class=\"status\">Status: <strong>{}</strong></p>\n\
             <p class=\"provides\">{}</p>\n",
            cls,
            esc(&o.reference),
            esc(&o.title),
            o.coverage.label(),
            esc_multiline(&o.provides)
        ));
        if let Some(gap) = &o.gap {
            body.push_str(&format!(
                "<p class=\"gap\"><strong>Gap / not covered:</strong> {}</p>\n",
                esc(gap)
            ));
        }
        body.push_str("</section>\n");
    }

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>Compliance Evidence Mapping &mdash; {}</title>\n<style>\n{}\n</style>\n\
         </head>\n<body>\n{}</body>\n</html>\n",
        esc(&f.run_id),
        HTML_STYLE,
        body
    )
}

const HTML_STYLE: &str = "body{font-family:system-ui,-apple-system,sans-serif;max-width:52rem;\
margin:2rem auto;padding:0 1rem;line-height:1.5;color:#1a1a1a}\
code{background:#f2f2f2;padding:.1em .3em;border-radius:3px;font-size:.9em;word-break:break-all}\
.verify.pass{border-left:4px solid #2e7d32;background:#edf7ed;padding:.5rem 1rem}\
.verify.fail{border-left:4px solid #c62828;background:#fdecea;padding:.5rem 1rem}\
.disclaimer{border:1px solid #999;background:#fafafa;padding:.5rem 1rem;font-size:.9em}\
section.ob{border:1px solid #ddd;border-radius:6px;padding:.5rem 1rem;margin:1rem 0}\
section.ob.notcovered{border-color:#c62828}section.ob.partial{border-color:#f9a825}\
section.ob.provided{border-color:#2e7d32}.status strong{text-transform:uppercase}\
.gap{color:#a11}";

/// HTML-escape a string for safe interpolation into element content.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Like `esc`, but turns newlines into `<br>` so multi-line `provides`
/// text keeps its line breaks in HTML.
fn esc_multiline(s: &str) -> String {
    esc(s).replace('\n', "<br>\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::evidence::EvidenceBundleBuilder;
    use crate::audit::log::{AuditEvent, AuditLog};
    use std::path::Path;

    /// Build a plaintext bundle (no retention metadata, no approvals) that
    /// verifies cleanly. Mirrors the constructions in verify.rs tests.
    fn build_bundle(dir: &Path) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-report-001", "report-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"report"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("s1", "result", r#"{"value":1}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "out".into(),
            duration_ms: 5,
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 6,
        });
        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn framework_and_format_parse() {
        assert_eq!(
            ComplianceFramework::parse("eu-ai-act").unwrap(),
            ComplianceFramework::EuAiAct
        );
        assert_eq!(
            ComplianceFramework::parse("NIST").unwrap(),
            ComplianceFramework::Nist
        );
        assert_eq!(
            ComplianceFramework::parse("iso42001").unwrap(),
            ComplianceFramework::Iso42001
        );
        assert!(ComplianceFramework::parse("gdpr").is_err());
        assert_eq!(ReportFormat::parse("md").unwrap(), ReportFormat::Markdown);
        assert_eq!(ReportFormat::parse("HTML").unwrap(), ReportFormat::Html);
        assert!(ReportFormat::parse("pdf").is_err());
    }

    #[test]
    fn eu_ai_act_report_maps_real_facts_and_flags_retention() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_bundle(dir.path());
        let bundle_dir = dir.path().join("run-report-001");

        let report = generate_report(
            &bundle_dir,
            ComplianceFramework::EuAiAct,
            ReportFormat::Markdown,
        )
        .unwrap();

        // Verified clean bundle.
        assert!(
            report.contains("Verification: PASSED"),
            "expected PASSED, got:\n{report}"
        );
        // The report cites the bundle's REAL identifiers.
        assert!(report.contains("run-report-001"), "missing run_id");
        assert!(
            report.contains(&manifest.audit_log_hash),
            "missing real audit_log_hash"
        );
        // Names the record-keeping article.
        assert!(report.contains("Art. 12"), "missing Art. 12");
        // Flags the missing retention policy against Art. 19.
        assert!(
            report.contains("NOT DECLARED") && report.contains("Art. 19"),
            "retention gap not flagged:\n{report}"
        );
        // Honest about being a mapping, not a certificate.
        assert!(
            report.contains("NOT a certificate of compliance"),
            "disclaimer missing"
        );
    }

    #[test]
    fn tampered_bundle_report_says_verification_failed() {
        let dir = tempfile::tempdir().unwrap();
        build_bundle(dir.path());
        let bundle_dir = dir.path().join("run-report-001");

        // Tamper a covered file: manifest still parses, but verify fails on
        // the checksum mismatch.
        std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"EVIL"}"#).unwrap();

        let report = generate_report(
            &bundle_dir,
            ComplianceFramework::EuAiAct,
            ReportFormat::Markdown,
        )
        .unwrap();

        assert!(
            report.contains("Verification: FAILED"),
            "tampered bundle must report FAILED:\n{report}"
        );
        assert!(
            report.contains("checksum mismatch"),
            "expected the checksum error surfaced in the report"
        );
        assert!(
            report.contains("cannot be trusted") || report.contains("CANNOT BE TRUSTED"),
            "expected a loud untrusted-evidence warning"
        );
    }

    #[test]
    fn nist_and_iso_reports_render() {
        let dir = tempfile::tempdir().unwrap();
        build_bundle(dir.path());
        let bundle_dir = dir.path().join("run-report-001");

        let nist = generate_report(
            &bundle_dir,
            ComplianceFramework::Nist,
            ReportFormat::Markdown,
        )
        .unwrap();
        assert!(nist.contains("MEASURE 2"), "NIST MEASURE mapping missing");
        assert!(nist.contains("MANAGE"), "NIST MANAGE mapping missing");

        let iso = generate_report(
            &bundle_dir,
            ComplianceFramework::Iso42001,
            ReportFormat::Html,
        )
        .unwrap();
        assert!(iso.starts_with("<!DOCTYPE html>"), "HTML doctype missing");
        assert!(iso.contains("Clause 7.5"), "ISO clause mapping missing");
        assert!(iso.contains("run-report-001"), "run_id missing in HTML");
    }

    #[test]
    fn report_maps_human_oversight_when_approvals_present() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-report-appr", "appr-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"appr"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::ApprovalRequested {
            step_id: "review".into(),
            role: "compliance-officer".into(),
        });
        audit.append(AuditEvent::ApprovalGranted {
            step_id: "review".into(),
            approver: "alice".into(),
        });
        builder.finalize(&audit).unwrap();
        let bundle_dir = dir.path().join("run-report-appr");

        let report = generate_report(
            &bundle_dir,
            ComplianceFramework::EuAiAct,
            ReportFormat::Markdown,
        )
        .unwrap();
        // Art. 14 human oversight is now evidenced, not N/A.
        assert!(report.contains("Art. 14"));
        assert!(
            report.contains("GRANTED by `alice`"),
            "approval record not surfaced:\n{report}"
        );
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_report(
            dir.path(),
            ComplianceFramework::EuAiAct,
            ReportFormat::Markdown,
        )
        .unwrap_err();
        assert!(err.contains("manifest.json"), "got: {err}");
    }
}
