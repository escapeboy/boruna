use std::fs;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use boruna_bytecode::Module;
use boruna_framework::runtime::AppMessage;
use boruna_framework::testing::TestHarness;
use boruna_framework::validate::AppValidator;
use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::repair::{RepairStrategy, RepairTool};
use boruna_tooling::trace2tests;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy, ReplayHandler};
use boruna_vm::replay::EventLog;
use boruna_vm::vm::Vm;

#[cfg(feature = "serve")]
mod serve;

#[derive(Parser)]
#[command(
    name = "boruna",
    about = "Boruna — deterministic, capability-safe language"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a .ax source file to bytecode.
    Compile {
        /// Source file path (.ax)
        file: PathBuf,
        /// Output file path (.axbc). Defaults to same name with .axbc extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Run a .ax source file or bytecode file.
    Run {
        /// File path (.ax or .axbc)
        file: PathBuf,
        /// Capability policy: "allow-all", "deny-all", or a JSON policy file.
        #[arg(short, long, default_value = "allow-all")]
        policy: String,
        /// Maximum execution steps.
        #[arg(long, default_value = "10000000")]
        max_steps: u64,
        /// Record execution events to this file.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Use real HTTP handler for net.fetch (requires `http` feature).
        #[arg(long)]
        live: bool,
    },
    /// Run with execution tracing enabled.
    Trace {
        /// File path (.ax or .axbc)
        file: PathBuf,
    },
    /// Replay execution from a recorded event log.
    Replay {
        /// Bytecode file (.axbc)
        file: PathBuf,
        /// Event log file (JSON)
        log: PathBuf,
    },
    /// Inspect a bytecode file.
    Inspect {
        /// Bytecode file (.axbc)
        file: PathBuf,
    },
    /// Dump the AST of a .ax source file.
    Ast {
        /// Source file path (.ax)
        file: PathBuf,
    },
    /// Framework commands.
    #[command(subcommand)]
    Framework(FrameworkCommand),
    /// Language tooling commands (diagnostics, repair).
    #[command(subcommand)]
    Lang(LangCommand),
    /// Trace-to-test tools (record, generate, run, minimize).
    #[command(subcommand)]
    Trace2tests(Trace2TestsCommand),
    /// Template tools (list, apply, validate).
    #[command(subcommand)]
    Template(TemplateCommand),
    /// Workflow execution and validation.
    #[command(subcommand)]
    Workflow(WorkflowCommand),
    /// Evidence bundle inspection and verification.
    #[command(subcommand)]
    Evidence(EvidenceCommand),
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// List available templates.
    List {
        /// Templates directory (defaults to ./templates).
        #[arg(long, default_value = "templates")]
        dir: PathBuf,
    },
    /// Apply a template with arguments.
    Apply {
        /// Template name.
        name: String,
        /// Templates directory (defaults to ./templates).
        #[arg(long, default_value = "templates")]
        dir: PathBuf,
        /// Template arguments as key=value pairs (comma-separated).
        #[arg(long)]
        args: String,
        /// Output file path.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Validate that output compiles.
        #[arg(long)]
        validate: bool,
    },
}

#[derive(Subcommand)]
enum LangCommand {
    /// Check a source file and report diagnostics.
    Check {
        /// Source file (.ax)
        file: PathBuf,
        /// Output diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Write JSON diagnostics to this file.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Repair a source file using diagnostic suggestions.
    Repair {
        /// Source file (.ax) to repair.
        file: PathBuf,
        /// Diagnostics JSON file (if not provided, runs check first).
        #[arg(long)]
        from: Option<PathBuf>,
        /// Strategy: "best" (default), "all", or a specific patch ID.
        #[arg(long, default_value = "best")]
        apply: String,
    },
}

#[derive(Subcommand)]
enum Trace2TestsCommand {
    /// Record an execution trace from a framework app.
    Record {
        /// Source file (.ax).
        file: PathBuf,
        /// Messages to send as "tag:payload" pairs (comma-separated).
        #[arg(short, long)]
        messages: String,
        /// Output trace file (JSON).
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Generate a test spec from a recorded trace.
    Generate {
        /// Trace file (JSON).
        #[arg(long)]
        trace: PathBuf,
        /// Test name.
        #[arg(long, default_value = "regression_test")]
        name: String,
        /// Output test spec file (JSON).
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run a test spec against source code.
    Run {
        /// Test spec file (JSON).
        #[arg(long)]
        spec: PathBuf,
        /// Source file (.ax). If not provided, uses source_file from spec.
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Minimize a failing trace using delta debugging.
    Minimize {
        /// Trace file (JSON).
        #[arg(long)]
        trace: PathBuf,
        /// Source file (.ax).
        #[arg(long)]
        source: PathBuf,
        /// Predicate: "panic" (default) or external command.
        #[arg(long, default_value = "panic")]
        predicate: String,
        /// Output minimized trace file.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum WorkflowCommand {
    /// Validate a workflow definition directory.
    Validate {
        /// Workflow directory containing workflow.json.
        dir: PathBuf,
    },
    /// Run a workflow.
    Run {
        /// Workflow directory containing workflow.json.
        dir: PathBuf,
        /// Capability policy: "allow-all", "deny-all", or a JSON policy file.
        #[arg(short, long, default_value = "allow-all")]
        policy: String,
        /// Record evidence bundle for this run.
        #[arg(long)]
        record: bool,
        /// Evidence output directory (defaults to <dir>/evidence/).
        #[arg(long)]
        evidence_dir: Option<PathBuf>,
        /// Use real HTTP handler for net.fetch (requires `http` feature).
        #[arg(long)]
        live: bool,
    },
}

#[derive(Subcommand)]
enum EvidenceCommand {
    /// Verify an evidence bundle for integrity.
    Verify {
        /// Evidence bundle directory.
        dir: PathBuf,
    },
    /// Inspect an evidence bundle's manifest.
    Inspect {
        /// Evidence bundle directory.
        dir: PathBuf,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FrameworkCommand {
    /// Create a new framework app from template.
    New {
        /// Application name
        name: String,
        /// Output directory (defaults to current dir)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Validate a .ax file conforms to the App protocol.
    Validate {
        /// Source file (.ax)
        file: PathBuf,
    },
    /// Run a framework app interactively with messages.
    Test {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send as "tag:payload" pairs (comma-separated).
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Inspect framework app state after running messages.
    InspectState {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send before inspecting.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Simulate a sequence of messages and display state transitions.
    Simulate {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages as "tag:payload" pairs (comma-separated).
        messages: String,
    },
    /// Print App contract summary (State, Messages, Effects) — machine-readable.
    Inspect {
        /// Source file (.ax)
        file: PathBuf,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Structured diagnostics output (JSON).
    Diag {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send before diagnostics.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Run with tracing and print a stable hash of the trace.
    TraceHash {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Replay a recorded cycle log and verify determinism.
    Replay {
        /// Source file (.ax)
        file: PathBuf,
        /// Cycle log file (JSON).
        log: PathBuf,
    },
    /// Serve a framework app as a web page (requires --features serve).
    #[cfg(feature = "serve")]
    Serve {
        /// Source file (.ax)
        file: PathBuf,
        /// Port to listen on.
        #[arg(short, long, default_value = "3000")]
        port: u16,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Compile { file, output } => {
            let source = fs::read_to_string(&file)?;
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".into());
            let module = boruna_compiler::compile(&name, &source)?;
            let out_path = output.unwrap_or_else(|| file.with_extension("axbc"));
            let bytes = module.to_bytes()?;
            fs::write(&out_path, bytes)?;
            println!("compiled {} -> {}", file.display(), out_path.display());
            println!(
                "  {} functions, {} constants, {} types",
                module.functions.len(),
                module.constants.len(),
                module.types.len()
            );
        }
        Command::Run {
            file,
            policy,
            max_steps,
            record,
            live,
        } => {
            let module = load_module(&file)?;
            let gateway = make_gateway(&policy, live)?;
            let mut vm = Vm::new(module, gateway);
            vm.set_max_steps(max_steps);

            match vm.run() {
                Ok(result) => {
                    println!("{result}");
                    if !vm.ui_output.is_empty() {
                        println!("\n--- UI Output ---");
                        for tree in &vm.ui_output {
                            let json = serde_json::to_string_pretty(tree)?;
                            println!("{json}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    process::exit(1);
                }
            }

            if let Some(log_path) = record {
                let json = vm
                    .event_log()
                    .to_json()
                    .map_err(|e| format!("failed to serialize event log: {e}"))?;
                fs::write(&log_path, json)?;
                println!("events recorded to {}", log_path.display());
            }

            println!("steps: {}", vm.step_count());
        }
        Command::Trace { file } => {
            let module = load_module(&file)?;
            let gateway = CapabilityGateway::new(Policy::allow_all());
            let mut vm = Vm::new(module, gateway);
            vm.trace_enabled = true;

            match vm.run() {
                Ok(result) => println!("result: {result}"),
                Err(e) => eprintln!("runtime error: {e}"),
            }

            println!("\n--- Trace ({} steps) ---", vm.step_count());
            for entry in &vm.trace {
                println!("  {entry}");
            }
        }
        Command::Replay { file, log } => {
            let module = load_module(&file)?;
            let log_json = fs::read_to_string(&log)?;
            let original_log =
                EventLog::from_json(&log_json).map_err(|e| format!("invalid event log: {e}"))?;

            let results = original_log.capability_results();
            let handler = Box::new(ReplayHandler::new(results));
            let gateway = CapabilityGateway::with_handler(Policy::allow_all(), handler);
            let mut vm = Vm::new(module, gateway);

            match vm.run() {
                Ok(result) => println!("replay result: {result}"),
                Err(e) => eprintln!("replay error: {e}"),
            }

            let replay_result =
                boruna_vm::replay::ReplayEngine::verify(&original_log, vm.event_log());
            println!("replay verification: {replay_result:?}");
        }
        Command::Inspect { file } => {
            let module = load_module(&file)?;
            println!("Module: {}", module.name);
            println!("Version: {}", module.version);
            println!("Entry: function #{}", module.entry);
            println!("\nTypes ({}):", module.types.len());
            for (i, t) in module.types.iter().enumerate() {
                println!("  #{i}: {} {:?}", t.name, t.kind);
            }
            println!("\nConstants ({}):", module.constants.len());
            for (i, c) in module.constants.iter().enumerate() {
                println!("  #{i}: {c}");
            }
            println!("\nFunctions ({}):", module.functions.len());
            for (i, f) in module.functions.iter().enumerate() {
                println!(
                    "  #{i}: {}(arity={}, locals={}, ops={})",
                    f.name,
                    f.arity,
                    f.locals,
                    f.code.len()
                );
                if !f.capabilities.is_empty() {
                    let caps: Vec<_> = f.capabilities.iter().map(|c| c.name()).collect();
                    println!("      capabilities: {}", caps.join(", "));
                }
                for (j, op) in f.code.iter().enumerate() {
                    println!("      {j:04}: {op:?}");
                }
            }
        }
        Command::Ast { file } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;
            let json = serde_json::to_string_pretty(&program)?;
            println!("{json}");
        }
        Command::Framework(fw) => run_framework(fw)?,
        Command::Lang(lang) => run_lang(lang)?,
        Command::Trace2tests(t2t) => run_trace2tests(t2t)?,
        Command::Template(tmpl) => run_template(tmpl)?,
        Command::Workflow(wf) => run_workflow(wf)?,
        Command::Evidence(ev) => run_evidence(ev)?,
    }
    Ok(())
}

fn run_lang(cmd: LangCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        LangCommand::Check { file, json, output } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();
            let ds = DiagnosticCollector::new(&file_str, &source).collect();

            if json || output.is_some() {
                let json_str = ds.to_json();
                if let Some(out_path) = output {
                    fs::write(&out_path, &json_str)?;
                    println!("diagnostics written to {}", out_path.display());
                } else {
                    println!("{json_str}");
                }
            } else {
                let human = ds.to_human();
                if human.is_empty() {
                    println!("no diagnostics");
                } else {
                    print!("{human}");
                }
            }

            if ds.has_errors() {
                process::exit(1);
            }
        }
        LangCommand::Repair { file, from, apply } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();

            // Get diagnostics: from file or run check
            let ds = if let Some(diag_path) = from {
                let json = fs::read_to_string(&diag_path)?;
                serde_json::from_str(&json).map_err(|e| format!("invalid diagnostics JSON: {e}"))?
            } else {
                DiagnosticCollector::new(&file_str, &source).collect()
            };

            let (strategy, specific_id) = match apply.as_str() {
                "best" => (RepairStrategy::Best, None),
                "all" => (RepairStrategy::All, None),
                id => (RepairStrategy::ById, Some(id.to_string())),
            };

            let (repaired, result) =
                RepairTool::repair(&file_str, &source, &ds, strategy, specific_id.as_deref());

            if result.applied.is_empty() {
                println!("no patches applied");
                if !result.skipped.is_empty() {
                    for s in &result.skipped {
                        println!("  skipped {}: {}", s.diagnostic_id, s.reason);
                    }
                }
            } else {
                // Write repaired source
                fs::write(&file, &repaired)?;
                println!("applied {} patches:", result.applied.len());
                for a in &result.applied {
                    println!("  [{}] {}: {}", a.diagnostic_id, a.patch_id, a.description);
                }
                println!(
                    "diagnostics: {} -> {}",
                    result.diagnostics_before, result.diagnostics_after
                );
                if result.verify_passed {
                    println!("verify: PASS");
                } else {
                    println!("verify: FAIL (remaining issues)");
                }
            }
        }
    }
    Ok(())
}

fn run_trace2tests(cmd: Trace2TestsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Trace2TestsCommand::Record {
            file,
            messages,
            out,
        } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();

            let msgs: Vec<AppMessage> = messages
                .split(',')
                .map(|s| {
                    let (tag, payload) = parse_message(s);
                    AppMessage::new(tag, payload)
                })
                .collect();

            let trace = trace2tests::record_trace(&source, &file_str, msgs)
                .map_err(|e| format!("record failed: {e}"))?;
            let json = serde_json::to_string_pretty(&trace)?;
            fs::write(&out, json)?;

            println!(
                "recorded {} cycles to {}",
                trace.cycles.len(),
                out.display()
            );
            println!("trace hash: {}", trace.trace_hash);
        }
        Trace2TestsCommand::Generate { trace, name, out } => {
            let trace_json = fs::read_to_string(&trace)?;
            let trace_file: trace2tests::TraceFile = serde_json::from_str(&trace_json)?;

            let spec = trace2tests::generate_test(&trace_file, &name);
            let json = serde_json::to_string_pretty(&spec)?;
            fs::write(&out, json)?;

            println!("generated test spec: {}", out.display());
            println!(
                "  {} messages, {} assertions",
                spec.messages.len(),
                spec.assertions.len()
            );
        }
        Trace2TestsCommand::Run { spec, source } => {
            let spec_json = fs::read_to_string(&spec)?;
            let test_spec: trace2tests::TestSpec = serde_json::from_str(&spec_json)?;

            let source_path = source.unwrap_or_else(|| PathBuf::from(&test_spec.source_file));
            let source_code = fs::read_to_string(&source_path)?;

            let result = trace2tests::run_test(&test_spec, &source_code);

            if result.passed {
                println!("PASS: {}", test_spec.name);
                for ar in &result.assertion_results {
                    println!("  [ok] {}", ar.kind);
                }
            } else {
                println!("FAIL: {}", test_spec.name);
                if let Some(err) = &result.error {
                    println!("  error: {err}");
                }
                for ar in &result.assertion_results {
                    let status = if ar.passed { "ok" } else { "FAIL" };
                    println!(
                        "  [{status}] {}: expected={}, actual={}",
                        ar.kind, ar.expected, ar.actual
                    );
                }
                process::exit(1);
            }
        }
        Trace2TestsCommand::Minimize {
            trace,
            source,
            predicate,
            out,
        } => {
            let trace_json = fs::read_to_string(&trace)?;
            let trace_file: trace2tests::TraceFile = serde_json::from_str(&trace_json)?;
            let source_code = fs::read_to_string(&source)?;

            let messages: Vec<trace2tests::TraceMessage> = trace_file
                .cycles
                .iter()
                .map(|c| c.message.clone())
                .collect();

            let original_len = messages.len();

            let minimal = match predicate.as_str() {
                "panic" => trace2tests::minimize_trace(
                    &source_code,
                    &messages,
                    &trace2tests::predicate_runtime_error,
                ),
                _ => {
                    // External command predicate
                    let cmd_str = predicate.clone();
                    let pred = move |_src: &str, msgs: &[trace2tests::TraceMessage]| {
                        external_predicate(&cmd_str, &trace_file.source_file, msgs)
                    };
                    trace2tests::minimize_trace(&source_code, &messages, &pred)
                }
            };

            println!("minimized: {} -> {} messages", original_len, minimal.len());

            // Record the minimal trace
            let app_msgs = trace2tests::messages_to_app(&minimal);
            let file_str = source.display().to_string();
            let minimal_trace = trace2tests::record_trace(&source_code, &file_str, app_msgs)
                .map_err(|e| format!("re-record failed: {e}"))?;
            let json = serde_json::to_string_pretty(&minimal_trace)?;

            let out_path = out.unwrap_or_else(|| {
                let stem = trace.with_extension("");
                PathBuf::from(format!("{}.min.json", stem.display()))
            });

            fs::write(&out_path, json)?;
            println!("minimized trace: {}", out_path.display());
            println!("trace hash: {}", minimal_trace.trace_hash);
        }
    }
    Ok(())
}

fn external_predicate(
    command: &str,
    source_file: &str,
    messages: &[trace2tests::TraceMessage],
) -> trace2tests::PredicateOutcome {
    let temp_file = match tempfile::NamedTempFile::new() {
        Ok(f) => f,
        Err(_) => return trace2tests::PredicateOutcome::Unresolved,
    };
    let temp_path = temp_file.path().to_path_buf();
    let trace_data = serde_json::json!({
        "source_file": source_file,
        "messages": messages,
    });
    if fs::write(
        &temp_path,
        serde_json::to_string(&trace_data).unwrap_or_default(),
    )
    .is_err()
    {
        return trace2tests::PredicateOutcome::Unresolved;
    }

    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return trace2tests::PredicateOutcome::Unresolved;
    }

    let output = std::process::Command::new(parts[0])
        .args(&parts[1..])
        .arg(temp_path.to_string_lossy().as_ref())
        .output();

    match output {
        Ok(o) if o.status.success() => trace2tests::PredicateOutcome::Pass,
        Ok(_) => trace2tests::PredicateOutcome::Fail,
        Err(_) => trace2tests::PredicateOutcome::Unresolved,
    }
}

fn run_framework(cmd: FrameworkCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        FrameworkCommand::New { name, dir } => {
            let base = dir.unwrap_or_else(|| PathBuf::from("."));
            let app_dir = base.join(&name);
            fs::create_dir_all(&app_dir)?;

            let source = format!(
                r#"// {name} — Boruna Framework App

type State {{ value: Int }}
type Msg {{ tag: String, payload: Int }}
type Effect {{ kind: String, payload: String, callback_tag: String }}
type UpdateResult {{ state: State, effects: List<Effect> }}
type UINode {{ tag: String, text: String }}

fn init() -> State {{
    State {{ value: 0 }}
}}

fn update(state: State, msg: Msg) -> UpdateResult {{
    UpdateResult {{
        state: State {{ value: state.value + msg.payload }},
        effects: [],
    }}
}}

fn view(state: State) -> UINode {{
    UINode {{ tag: "text", text: "value" }}
}}

fn main() -> Int {{
    let s: State = init()
    s.value
}}
"#
            );

            let file_path = app_dir.join(format!("{name}.ax"));
            fs::write(&file_path, source)?;
            println!("created framework app: {}", file_path.display());
            println!("  run: boruna framework validate {}", file_path.display());
            println!("  run: boruna run {}", file_path.display());
        }
        FrameworkCommand::Validate { file } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;

            match AppValidator::validate(&program) {
                Ok(result) => {
                    println!("valid App protocol");
                    println!(
                        "  init:     {}",
                        if result.has_init { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  update:   {}",
                        if result.has_update { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  view:     {}",
                        if result.has_view { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  policies: {}",
                        if result.has_policies {
                            "yes"
                        } else {
                            "none (using defaults)"
                        }
                    );
                    if let Some(t) = &result.state_type {
                        println!("  state type: {t}");
                    }
                    if let Some(t) = &result.message_type {
                        println!("  message type: {t}");
                    }
                }
                Err(e) => {
                    eprintln!("validation failed: {e}");
                    process::exit(1);
                }
            }
        }
        FrameworkCommand::Test { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            println!("init state: {}", harness.state());

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    let msg = AppMessage::new(tag, payload);
                    match harness.send(msg) {
                        Ok((state, effects)) => {
                            println!("cycle {}: state={}", harness.cycle(), state);
                            if !effects.is_empty() {
                                println!("  effects: {}", effects.len());
                                for e in &effects {
                                    println!("    - {:?} -> {}", e.kind, e.callback_tag);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("error at cycle {}: {e}", harness.cycle());
                            process::exit(1);
                        }
                    }
                }
            }

            println!("\nfinal state: {}", harness.state());
            println!("cycles: {}", harness.cycle());

            // Show view
            match harness.view() {
                Ok(ui) => println!("view: {ui}"),
                Err(e) => eprintln!("view error: {e}"),
            }
        }
        FrameworkCommand::InspectState { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    harness.send(AppMessage::new(tag, payload))?;
                }
            }

            println!("--- State Inspection ---");
            println!("cycle: {}", harness.cycle());
            println!("state: {}", harness.state());
            println!("\nJSON snapshot:");
            println!("{}", harness.snapshot());

            if harness.cycle() > 0 {
                println!("\n--- Diff from init ---");
                let log = harness.cycle_log();
                if let (Some(first), Some(last)) = (log.first(), log.last()) {
                    let diffs = boruna_framework::state::StateMachine::diff_values(
                        &first.state_before,
                        &last.state_after,
                    );
                    if diffs.is_empty() {
                        println!("  no changes");
                    } else {
                        for d in &diffs {
                            println!(
                                "  field[{}]: {} -> {}",
                                d.field_index, d.old_value, d.new_value
                            );
                        }
                    }
                }
            }
        }
        FrameworkCommand::Simulate { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            println!("=== Simulation ===");
            println!("init: {}", harness.state());
            println!();

            for (i, msg_str) in messages.split(',').enumerate() {
                let (tag, payload) = parse_message(msg_str);
                let msg = AppMessage::new(tag.clone(), payload);
                match harness.send(msg) {
                    Ok((state, effects)) => {
                        println!("step {}: msg=\"{tag}\"", i + 1);
                        println!("  state: {state}");
                        if !effects.is_empty() {
                            for e in &effects {
                                println!("  effect: {:?}", e.kind);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("step {} failed: {e}", i + 1);
                        process::exit(1);
                    }
                }
            }

            println!("\n=== Final ===");
            println!("state: {}", harness.state());
            println!("cycles: {}", harness.cycle());
        }
        FrameworkCommand::Inspect { file, json } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;
            let result = AppValidator::validate(&program)?;

            if json {
                let output = serde_json::json!({
                    "file": file.display().to_string(),
                    "has_init": result.has_init,
                    "has_update": result.has_update,
                    "has_view": result.has_view,
                    "has_policies": result.has_policies,
                    "state_type": result.state_type,
                    "message_type": result.message_type,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("=== App Contract ===");
                println!("file: {}", file.display());
                println!(
                    "init:     {}",
                    if result.has_init { "yes" } else { "MISSING" }
                );
                println!(
                    "update:   {}",
                    if result.has_update { "yes" } else { "MISSING" }
                );
                println!(
                    "view:     {}",
                    if result.has_view { "yes" } else { "MISSING" }
                );
                println!(
                    "policies: {}",
                    if result.has_policies {
                        "yes"
                    } else {
                        "none (defaults)"
                    }
                );
                if let Some(t) = &result.state_type {
                    println!("state type:   {t}");
                }
                if let Some(t) = &result.message_type {
                    println!("message type: {t}");
                }
            }
        }
        FrameworkCommand::Diag { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let harness_result = TestHarness::from_source(&source);

            let mut diag = serde_json::json!({
                "file": file.display().to_string(),
                "status": "ok",
            });

            match harness_result {
                Ok(mut harness) => {
                    diag["init_state"] = serde_json::json!(format!("{}", harness.state()));

                    let mut cycles = Vec::new();
                    let mut errors = Vec::new();

                    if let Some(msgs) = messages {
                        for msg_str in msgs.split(',') {
                            let (tag, payload) = parse_message(msg_str);
                            match harness.send(AppMessage::new(tag.clone(), payload)) {
                                Ok((state, effects)) => {
                                    let effect_kinds: Vec<&str> =
                                        effects.iter().map(|e| e.kind.as_str()).collect();
                                    cycles.push(serde_json::json!({
                                        "cycle": harness.cycle(),
                                        "message": tag,
                                        "state": format!("{state}"),
                                        "effects": effect_kinds,
                                    }));
                                }
                                Err(e) => {
                                    errors.push(boruna_framework::policy::error_to_json(&e));
                                    break;
                                }
                            }
                        }
                    }

                    diag["cycles"] = serde_json::json!(cycles);
                    diag["final_state"] = serde_json::json!(format!("{}", harness.state()));
                    diag["final_cycle"] = serde_json::json!(harness.cycle());
                    diag["snapshot"] = serde_json::json!(harness.snapshot());

                    if !errors.is_empty() {
                        diag["status"] = serde_json::json!("error");
                        diag["errors"] = serde_json::json!(errors);
                    }
                }
                Err(e) => {
                    diag["status"] = serde_json::json!("error");
                    diag["error"] = serde_json::json!(format!("{e}"));
                }
            }

            println!("{}", serde_json::to_string_pretty(&diag)?);
        }
        FrameworkCommand::TraceHash { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    harness.send(AppMessage::new(tag, payload))?;
                }
            }

            // Build a stable trace string from the cycle log
            let mut trace = String::new();
            for r in harness.cycle_log() {
                trace.push_str(&format!(
                    "c{}:{}:{},before={},after={},fx=[{}]\n",
                    r.cycle,
                    r.message.tag,
                    r.message.payload,
                    r.state_before,
                    r.state_after,
                    r.effects
                        .iter()
                        .map(|e| e.kind.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ));
            }

            // SHA-256 hash (consistent with trace2tests)
            let hash = boruna_tooling::trace2tests::sha256_hex(&trace);

            println!("{}", hash);
            println!("cycles: {}", harness.cycle());
            println!("state: {}", harness.state());
        }
        #[cfg(feature = "serve")]
        FrameworkCommand::Serve { file, port } => {
            serve::run_serve(file, port)?;
        }
        FrameworkCommand::Replay { file, log } => {
            let source = fs::read_to_string(&file)?;
            let log_json = fs::read_to_string(&log)?;

            // Parse the cycle log
            let recorded: Vec<serde_json::Value> = serde_json::from_str(&log_json)
                .map_err(|e| format!("invalid cycle log JSON: {e}"))?;

            let mut harness = TestHarness::from_source(&source)?;
            let mut mismatches = Vec::new();

            for (i, entry) in recorded.iter().enumerate() {
                let tag = entry["message"].as_str().unwrap_or("unknown");
                let payload_str = entry.get("payload").and_then(|v| v.as_str()).unwrap_or("0");
                let (_, payload) = parse_message(&format!("{tag}:{payload_str}"));

                match harness.send(AppMessage::new(tag, payload)) {
                    Ok((state, _)) => {
                        let expected_state = entry
                            .get("state_after")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let actual_state = format!("{state}");
                        if !expected_state.is_empty() && actual_state != expected_state {
                            mismatches.push(format!(
                                "cycle {}: expected state {}, got {}",
                                i + 1,
                                expected_state,
                                actual_state
                            ));
                        }
                    }
                    Err(e) => {
                        mismatches.push(format!("cycle {} failed: {e}", i + 1));
                        break;
                    }
                }
            }

            if mismatches.is_empty() {
                println!("replay: IDENTICAL");
                println!("cycles: {}", harness.cycle());
                println!("state: {}", harness.state());
            } else {
                println!("replay: DIVERGED");
                for m in &mismatches {
                    println!("  {m}");
                }
                process::exit(1);
            }
        }
    }
    Ok(())
}

/// Parse "tag:payload" into (tag, Value).
/// payload is parsed as Int if numeric, otherwise String.
fn parse_message(s: &str) -> (String, boruna_bytecode::Value) {
    let s = s.trim();
    if let Some(idx) = s.find(':') {
        let tag = s[..idx].to_string();
        let payload_str = s[idx + 1..].trim();
        let payload = if let Ok(n) = payload_str.parse::<i64>() {
            boruna_bytecode::Value::Int(n)
        } else {
            boruna_bytecode::Value::String(payload_str.to_string())
        };
        (tag, payload)
    } else {
        (s.to_string(), boruna_bytecode::Value::Int(0))
    }
}

fn load_module(path: &PathBuf) -> Result<Module, Box<dyn std::error::Error>> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    match ext {
        "axbc" => {
            let data = fs::read(path)?;
            Ok(Module::from_bytes(&data)?)
        }
        "ax" => {
            let source = fs::read_to_string(path)?;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".into());
            Ok(boruna_compiler::compile(&name, &source)?)
        }
        _ => Err(format!("unknown file extension: {ext}").into()),
    }
}

fn make_gateway(
    policy_str: &str,
    live: bool,
) -> Result<CapabilityGateway, Box<dyn std::error::Error>> {
    let policy = match policy_str {
        "allow-all" => Policy::allow_all(),
        "deny-all" => Policy::deny_all(),
        path => {
            let json = fs::read_to_string(path)?;
            serde_json::from_str(&json)?
        }
    };

    if live {
        #[cfg(feature = "http")]
        {
            let net_policy = policy.net_policy.clone().unwrap_or_default();
            return Ok(CapabilityGateway::with_handler(
                policy,
                Box::new(boruna_vm::http_handler::HttpHandler::new(net_policy)),
            ));
        }
        #[cfg(not(feature = "http"))]
        {
            eprintln!("warning: --live requires the `http` feature; falling back to mock handler");
        }
    }

    Ok(CapabilityGateway::new(policy))
}

fn run_workflow(cmd: WorkflowCommand) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_orchestrator::audit::{AuditEvent, AuditLog, EvidenceBundleBuilder};
    use boruna_orchestrator::workflow::{
        RunOptions, WorkflowDef, WorkflowRunner, WorkflowValidator,
    };

    match cmd {
        WorkflowCommand::Validate { dir } => {
            let def_path = dir.join("workflow.json");
            let json = fs::read_to_string(&def_path)
                .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
            let def: WorkflowDef =
                serde_json::from_str(&json).map_err(|e| format!("invalid workflow.json: {e}"))?;

            match WorkflowValidator::validate(&def) {
                Ok(()) => {
                    let order = WorkflowValidator::topological_order(&def)?;
                    println!("workflow '{}' v{} is valid", def.name, def.version);
                    println!("  steps: {}", def.steps.len());
                    println!("  edges: {}", def.edges.len());
                    println!("  execution order: {}", order.join(" -> "));
                }
                Err(errors) => {
                    eprintln!("validation failed:");
                    for err in &errors {
                        eprintln!("  {err}");
                    }
                    process::exit(1);
                }
            }
        }
        WorkflowCommand::Run {
            dir,
            policy,
            record,
            evidence_dir,
            live,
        } => {
            let def_path = dir.join("workflow.json");
            let json = fs::read_to_string(&def_path)
                .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
            let def: WorkflowDef =
                serde_json::from_str(&json).map_err(|e| format!("invalid workflow.json: {e}"))?;

            let policy_obj = match policy.as_str() {
                "allow-all" => Policy::allow_all(),
                "deny-all" => Policy::deny_all(),
                path => {
                    let pjson = fs::read_to_string(path)?;
                    serde_json::from_str(&pjson)?
                }
            };

            let options = RunOptions {
                policy: Some(policy_obj.clone()),
                record,
                workflow_dir: dir.display().to_string(),
                live,
            };

            let result = WorkflowRunner::run(&def, &options).map_err(|e| format!("{e}"))?;

            println!("workflow '{}' run: {:?}", def.name, result.status);
            println!("  run_id: {}", result.run_id);
            println!("  duration: {}ms", result.total_duration_ms);
            for (id, sr) in &result.step_results {
                println!("  step '{id}': {:?} ({}ms)", sr.status, sr.duration_ms);
                if let Some(err) = &sr.error {
                    println!("    error: {err}");
                }
            }

            if record {
                let ev_dir = evidence_dir.unwrap_or_else(|| dir.join("evidence"));
                let mut builder = EvidenceBundleBuilder::new(&ev_dir, &result.run_id, &def.name)?;

                builder.add_workflow_def(&json)?;
                let policy_json = serde_json::to_string_pretty(&policy_obj)?;
                builder.add_policy(&policy_json)?;

                // Build audit log from results
                let mut audit = AuditLog::new();
                audit.append(AuditEvent::WorkflowStarted {
                    workflow_hash: boruna_orchestrator::workflow::DataStore::hash_value(
                        &boruna_bytecode::Value::String(json.clone()),
                    ),
                    policy_hash: boruna_orchestrator::workflow::DataStore::hash_value(
                        &boruna_bytecode::Value::String(policy_json.clone()),
                    ),
                });

                for (id, sr) in &result.step_results {
                    match &sr.status {
                        boruna_orchestrator::workflow::StepStatus::Completed => {
                            audit.append(AuditEvent::StepCompleted {
                                step_id: id.clone(),
                                output_hash: sr.output_hash.clone().unwrap_or_default(),
                                duration_ms: sr.duration_ms,
                            });
                        }
                        boruna_orchestrator::workflow::StepStatus::Failed => {
                            audit.append(AuditEvent::StepFailed {
                                step_id: id.clone(),
                                error: sr.error.clone().unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }

                audit.append(AuditEvent::WorkflowCompleted {
                    result_hash: format!("{:?}", result.status),
                    total_duration_ms: result.total_duration_ms,
                });

                let manifest = builder.finalize(&audit)?;
                println!(
                    "\nevidence bundle: {}",
                    ev_dir.join(&result.run_id).display()
                );
                println!("  bundle_hash: {}", manifest.bundle_hash);
                println!("  audit_log_hash: {}", manifest.audit_log_hash);
                println!("  files: {}", manifest.file_checksums.len());
            }
        }
    }
    Ok(())
}

fn run_evidence(cmd: EvidenceCommand) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_orchestrator::audit::{evidence::BundleManifest, verify::verify_bundle};

    match cmd {
        EvidenceCommand::Verify { dir } => {
            let result = verify_bundle(&dir);
            if result.valid {
                println!("evidence bundle is VALID");
            } else {
                eprintln!("evidence bundle INVALID:");
                for err in &result.errors {
                    eprintln!("  {err}");
                }
                process::exit(1);
            }
        }
        EvidenceCommand::Inspect { dir, json } => {
            let manifest_path = dir.join("manifest.json");
            let manifest_json = fs::read_to_string(&manifest_path)
                .map_err(|e| format!("cannot read manifest: {e}"))?;
            let manifest: BundleManifest = serde_json::from_str(&manifest_json)
                .map_err(|e| format!("invalid manifest: {e}"))?;

            if json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                println!("=== Evidence Bundle ===");
                println!("run_id:        {}", manifest.run_id);
                println!("workflow:      {}", manifest.workflow_name);
                println!("started_at:    {}", manifest.started_at);
                println!("completed_at:  {}", manifest.completed_at);
                println!("bundle_hash:   {}", manifest.bundle_hash);
                println!("workflow_hash: {}", manifest.workflow_hash);
                println!("policy_hash:   {}", manifest.policy_hash);
                println!("audit_hash:    {}", manifest.audit_log_hash);
                println!("files: {}", manifest.file_checksums.len());
                for (name, hash) in &manifest.file_checksums {
                    println!("  {name}: {}", &hash[..16]);
                }
                println!("\nenv:");
                println!("  boruna: {}", manifest.env_fingerprint.boruna_version);
                println!(
                    "  os: {}/{}",
                    manifest.env_fingerprint.os, manifest.env_fingerprint.arch
                );
            }
        }
    }
    Ok(())
}

fn run_template(cmd: TemplateCommand) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_tooling::templates;

    match cmd {
        TemplateCommand::List { dir } => {
            let templates = templates::list_templates(&dir)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            if templates.is_empty() {
                println!("no templates found in {}", dir.display());
            } else {
                println!("available templates:");
                for t in &templates {
                    println!("  {} v{} — {}", t.name, t.version, t.description);
                    if !t.dependencies.is_empty() {
                        println!("    deps: {}", t.dependencies.join(", "));
                    }
                    if !t.capabilities.is_empty() {
                        println!("    caps: {}", t.capabilities.join(", "));
                    }
                }
            }
        }
        TemplateCommand::Apply {
            name,
            dir,
            args,
            out,
            validate,
        } => {
            // Parse args from "key1=val1,key2=val2"
            let mut arg_map = std::collections::BTreeMap::new();
            for pair in args.split(',') {
                let pair = pair.trim();
                if pair.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = pair.splitn(2, '=').collect();
                if parts.len() != 2 {
                    return Err(format!("invalid arg format: '{pair}' (expected key=value)").into());
                }
                arg_map.insert(parts[0].to_string(), parts[1].to_string());
            }

            let result = templates::apply_template(&dir, &name, &arg_map)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            if validate {
                templates::validate_template_output(&result.source)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
                println!("template output validates OK");
            }

            let output_path = out.unwrap_or_else(|| PathBuf::from(&result.output_file));
            fs::write(&output_path, &result.source)?;
            println!(
                "generated {} from template '{}'",
                output_path.display(),
                result.template_name
            );
            println!("  deps: {}", result.dependencies.join(", "));
            println!("  caps: {}", result.capabilities.join(", "));
        }
    }
    Ok(())
}
