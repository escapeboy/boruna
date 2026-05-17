//! `boruna size` — bytecode artifact cost report.
//!
//! Compiles a `.ax` source file and reports the size of the resulting
//! bytecode module: per-function opcode counts, module-wide totals, and the
//! serialized `.axbc` artifact byte count. Read-only — nothing is written to
//! disk. `--json` emits the report for agent consumption.

use serde::Serialize;

use boruna_bytecode::Module;

#[derive(Debug, Serialize)]
struct FunctionSize {
    name: String,
    arity: u8,
    locals: u16,
    op_count: usize,
    capability_count: usize,
}

#[derive(Debug, Serialize)]
struct Totals {
    function_count: usize,
    total_ops: usize,
    constants: usize,
    types: usize,
    globals: usize,
}

#[derive(Debug, Serialize)]
struct SizeReport {
    module: String,
    functions: Vec<FunctionSize>,
    totals: Totals,
    bytecode_bytes: usize,
    bytecode_format: &'static str,
}

/// Compile `source` (named `name`) and print its bytecode size report.
pub fn run(name: &str, source: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let module: Module = boruna_compiler::compile(name, source)?;

    let functions: Vec<FunctionSize> = module
        .functions
        .iter()
        .map(|f| FunctionSize {
            name: f.name.clone(),
            arity: f.arity,
            locals: f.locals,
            op_count: f.code.len(),
            capability_count: f.capabilities.len(),
        })
        .collect();

    let total_ops = functions.iter().map(|f| f.op_count).sum();
    let totals = Totals {
        function_count: module.functions.len(),
        total_ops,
        constants: module.constants.len(),
        types: module.types.len(),
        globals: module.globals.len(),
    };

    let bytecode_bytes = module.to_bytes()?.len();

    let report = SizeReport {
        module: module.name.clone(),
        functions,
        totals,
        bytecode_bytes,
        bytecode_format: "axbc",
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("module '{}' — bytecode size", report.module);
        println!(
            "  {:<24} {:>6} {:>7} {:>6} {:>6}",
            "FUNCTION", "ARITY", "LOCALS", "OPS", "CAPS"
        );
        for f in &report.functions {
            println!(
                "  {:<24} {:>6} {:>7} {:>6} {:>6}",
                f.name, f.arity, f.locals, f.op_count, f.capability_count
            );
        }
        println!(
            "  totals: {} functions, {} ops, {} constants, {} types, {} globals",
            report.totals.function_count,
            report.totals.total_ops,
            report.totals.constants,
            report.totals.types,
            report.totals.globals
        );
        println!(
            "  artifact: {} bytes ({} format)",
            report.bytecode_bytes, report.bytecode_format
        );
    }
    Ok(())
}
