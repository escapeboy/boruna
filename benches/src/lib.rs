//! Shared fixtures for the Boruna benchmark suite (sprint `W5-A`).
//!
//! The criterion benches and the smoke test both pull their `.ax`
//! source bodies and evidence-bundle scaffolding from this module so a
//! single change here flows everywhere. See `docs/PERFORMANCE.md` for
//! the documented baseline and budget commitments.

use std::collections::BTreeMap;
use std::path::Path;

use boruna_bytecode::Module;
use boruna_compiler::compile;
use boruna_orchestrator::audit::{AuditEvent, AuditLog, EvidenceBundleBuilder};
use boruna_tooling::templates::apply_template;
use boruna_vm::{CapabilityGateway, Policy, Vm};

/// Minimal program (~20 lines) — exercises the compiler's hot path
/// without pulling in pattern matching or records.
pub const SMALL_AX_SOURCE: &str = r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}

fn double(x: Int) -> Int {
    x + x
}

fn main() -> Int {
    let a: Int = 1
    let b: Int = 2
    let c: Int = add(a, b)
    let d: Int = double(c)
    d + 10
}
"#;

/// Medium program (~200 lines): records, pattern matching, recursion,
/// while loops, list ops. Roughly the shape of a stdlib helper module.
pub const MEDIUM_AX_SOURCE: &str = r#"
type Pair { a: Int, b: Int }
type Triple { x: Int, y: Int, z: Int }
type Tag { name: String, weight: Int }

fn add(a: Int, b: Int) -> Int { a + b }
fn sub(a: Int, b: Int) -> Int { a - b }
fn mul(a: Int, b: Int) -> Int { a * b }
fn neg(x: Int) -> Int { 0 - x }

fn abs(x: Int) -> Int {
    if x < 0 { neg(x) } else { x }
}

fn max2(a: Int, b: Int) -> Int {
    if a > b { a } else { b }
}

fn min2(a: Int, b: Int) -> Int {
    if a < b { a } else { b }
}

fn clamp(v: Int, lo: Int, hi: Int) -> Int {
    max2(lo, min2(v, hi))
}

fn sum_pair(p: Pair) -> Int {
    p.a + p.b
}

fn swap_pair(p: Pair) -> Pair {
    Pair { a: p.b, b: p.a }
}

fn shift_pair(p: Pair, dx: Int) -> Pair {
    Pair { ..p, a: p.a + dx }
}

fn triple_sum(t: Triple) -> Int {
    t.x + t.y + t.z
}

fn move_triple(t: Triple, dx: Int, dy: Int, dz: Int) -> Triple {
    Triple { x: t.x + dx, y: t.y + dy, z: t.z + dz }
}

fn fib(n: Int) -> Int {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}

fn factorial(n: Int) -> Int {
    let mut result: Int = 1
    let mut i: Int = n
    while i > 0 {
        result = result * i
        i = i - 1
    }
    result
}

fn pow(base: Int, exp: Int) -> Int {
    let mut acc: Int = 1
    let mut e: Int = exp
    while e > 0 {
        acc = acc * base
        e = e - 1
    }
    acc
}

fn gcd(a: Int, b: Int) -> Int {
    let mut x: Int = a
    let mut y: Int = b
    while y > 0 {
        let t: Int = y
        y = x - (x / y) * y
        x = t
    }
    x
}

fn classify(tag: Tag) -> String {
    match tag.name {
        "alpha" => "high",
        "beta" => "medium",
        "gamma" => "low",
        _ => "unknown",
    }
}

fn weight_of(tag: Tag) -> Int {
    match tag.name {
        "alpha" => 100,
        "beta" => 50,
        "gamma" => 10,
        _ => 0,
    }
}

fn promote(tag: Tag) -> Tag {
    Tag { ..tag, weight: tag.weight + 5 }
}

fn sum_n(n: Int) -> Int {
    let mut total: Int = 0
    let mut i: Int = 0
    while i < n {
        total = total + i
        i = i + 1
    }
    total
}

fn count_down(n: Int) -> Int {
    let mut x: Int = n
    while x > 0 { x = x - 1 }
    x
}

fn pipeline(seed: Int) -> Int {
    let a: Int = sum_n(seed)
    let b: Int = factorial(min2(seed, 8))
    let c: Int = pow(2, min2(seed, 10))
    let d: Int = gcd(a + 1, b + 1)
    a + b + c + d
}

fn main() -> Int {
    let p: Pair = Pair { a: 3, b: 4 }
    let q: Pair = swap_pair(p)
    let r: Pair = shift_pair(q, 5)
    let t: Triple = Triple { x: 1, y: 2, z: 3 }
    let u: Triple = move_triple(t, 10, 20, 30)
    let tag: Tag = Tag { name: "beta", weight: 50 }
    let promoted: Tag = promote(tag)
    let w: Int = weight_of(promoted)
    let f: Int = fib(10)
    let result: Int = sum_pair(r) + triple_sum(u) + w + f + pipeline(6)
    clamp(result, 0, 100000)
}
"#;

/// Build the crud-admin template with a stable set of args. Returns the
/// expanded `.ax` source ready for `compile()`.
pub fn render_crud_admin_template() -> String {
    let mut args = BTreeMap::new();
    args.insert("entity_name".into(), "products".into());
    args.insert("fields".into(), "name|price|sku|stock".into());
    let templates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates");
    let result = apply_template(&templates_dir, "crud-admin", &args)
        .expect("crud-admin template should exist and substitute");
    result.source
}

/// Compile to a module — used as the setup step for VM benchmarks.
pub fn compile_or_panic(name: &str, source: &str) -> Module {
    compile(name, source).unwrap_or_else(|e| panic!("compile {name} failed: {e}"))
}

/// Pure Int → Int loop. The bench varies `iterations` to capture
/// per-step throughput as the body length changes.
pub fn loop_program(iterations: i64) -> String {
    format!(
        r#"
fn main() -> Int {{
    let mut acc: Int = 0
    let mut i: Int = 0
    while i < {iterations} {{
        acc = acc + i
        i = i + 1
    }}
    acc
}}
"#
    )
}

/// Loop body that allocates a record on each iteration. Stresses the
/// VM allocator path.
pub fn loop_with_record_program(iterations: i64) -> String {
    format!(
        r#"
type Pair {{ a: Int, b: Int }}

fn step(p: Pair) -> Pair {{
    Pair {{ a: p.a + 1, b: p.b + p.a }}
}}

fn main() -> Int {{
    let mut p: Pair = Pair {{ a: 0, b: 0 }}
    let mut i: Int = 0
    while i < {iterations} {{
        p = step(p)
        i = i + 1
    }}
    p.a + p.b
}}
"#
    )
}

/// Loop body that exercises the call/return path (4-deep call chain
/// per iteration). Stand-in for "capability dispatch" workload — the
/// real `Capability` machinery is hand-coded only against `step_input`
/// in surface syntax, and step_input requires a workflow context. Pure
/// function dispatch shares the same hot opcode loop (Call/Return) and
/// is the closest surface-language analogue we can express.
pub fn loop_with_capability_program(iterations: i64) -> String {
    format!(
        r#"
fn inner(x: Int) -> Int {{
    x + 1
}}

fn middle(x: Int) -> Int {{
    inner(x) + inner(x + 1)
}}

fn outer(x: Int) -> Int {{
    middle(x) + middle(x + 2)
}}

fn main() -> Int {{
    let mut acc: Int = 0
    let mut i: Int = 0
    while i < {iterations} {{
        acc = acc + outer(i)
        i = i + 1
    }}
    acc
}}
"#
    )
}

/// Run a compiled module under an allow-all gateway and return the
/// final value. Panics on any VM error — benches expect inputs to
/// succeed.
pub fn run_module(module: Module) -> boruna_bytecode::Value {
    let gateway = CapabilityGateway::new(Policy::allow_all());
    let mut vm = Vm::new(module, gateway);
    vm.run().expect("vm.run should succeed for bench fixture")
}

/// Build an evidence bundle with `step_count` step outputs. Each step's
/// output is a small JSON blob; the audit log gets a started/completed
/// pair plus one StepCompleted per step. Returns the bundle directory
/// path so the verify bench can re-open it.
///
/// The `tempdir` handle must outlive the verify call; keep it on the
/// stack in the caller.
pub fn build_evidence_bundle(base: &Path, run_id: &str, step_count: usize) -> std::path::PathBuf {
    let mut builder =
        EvidenceBundleBuilder::new(base, run_id, "bench-workflow").expect("create bundle builder");
    builder
        .add_workflow_def(r#"{"name":"bench-workflow","steps":[]}"#)
        .expect("add workflow def");
    builder
        .add_policy(r#"{"schema_version":1,"rules":{},"default_allow":true}"#)
        .expect("add policy");

    let mut audit = AuditLog::new();
    audit.append(AuditEvent::WorkflowStarted {
        workflow_hash: "wf-hash".into(),
        policy_hash: "policy-hash".into(),
    });

    for i in 0..step_count {
        let step_id = format!("step{i}");
        let output = format!(
            r#"{{"index":{i},"value":"step-output-{i}","payload":"{}"}}"#,
            "x".repeat(64)
        );
        builder
            .add_step_output(&step_id, "result", &output)
            .expect("add step output");
        audit.append(AuditEvent::StepCompleted {
            step_id,
            output_hash: format!("hash-{i:040}"),
            duration_ms: 10,
        });
    }

    audit.append(AuditEvent::WorkflowCompleted {
        result_hash: "final-hash".into(),
        total_duration_ms: (step_count as u64) * 10,
    });

    let _manifest = builder.finalize(&audit).expect("finalize bundle");
    base.join(run_id)
}
