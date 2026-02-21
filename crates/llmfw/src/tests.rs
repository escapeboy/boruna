#[cfg(test)]
mod tests {
    use crate::effect::EffectKind;
    use crate::policy::PolicySet;
    use crate::runtime::{AppMessage, AppRuntime};
    use crate::state::StateMachine;
    use crate::testing::TestHarness;
    use crate::validate::AppValidator;
    use boruna_bytecode::Value;

    /// Minimal counter app source code.
    const COUNTER_APP: &str = r#"
type State { count: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { count: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    let new_count: Int = if msg.tag == "increment" {
        state.count + 1
    } else {
        if msg.tag == "decrement" {
            state.count - 1
        } else {
            state.count
        }
    }
    UpdateResult {
        state: State { count: new_count },
        effects: [],
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: "count" }
}
"#;

    /// App with effects.
    const EFFECT_APP: &str = r#"
type State { data: String }
type Msg { tag: String, payload: String }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { data: "" }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "fetch" {
        UpdateResult {
            state: state,
            effects: [Effect { kind: "http_request", payload: "https://example.com", callback_tag: "fetched" }],
        }
    } else {
        UpdateResult {
            state: State { data: msg.payload },
            effects: [],
        }
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: state.data }
}
"#;

    /// Invalid app (missing update).
    const INVALID_APP: &str = r#"
type State { x: Int }

fn init() -> State {
    State { x: 0 }
}

fn view(state: State) -> Int {
    state.x
}
"#;

    /// App with policies.
    const POLICY_APP: &str = r#"
type State { count: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }
type PolicySet { capabilities: List<String>, max_effects: Int, max_steps: Int }

fn init() -> State {
    State { count: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    UpdateResult {
        state: State { count: state.count + 1 },
        effects: [],
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: "count" }
}

fn policies() -> PolicySet {
    PolicySet { capabilities: ["net.fetch", "time.now"], max_effects: 10, max_steps: 1000000 }
}
"#;

    // --- Validator Tests ---

    #[test]
    fn test_validate_valid_app() {
        let tokens = boruna_compiler::lexer::lex(COUNTER_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let result = AppValidator::validate(&program);
        assert!(result.is_ok(), "validation failed: {result:?}");
        let result = result.unwrap();
        assert!(result.has_init);
        assert!(result.has_update);
        assert!(result.has_view);
    }

    #[test]
    fn test_validate_invalid_app() {
        let tokens = boruna_compiler::lexer::lex(INVALID_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let result = AppValidator::validate(&program);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_detects_missing_update() {
        let tokens = boruna_compiler::lexer::lex(INVALID_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let err = AppValidator::validate(&program).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("update"), "error should mention update: {msg}");
    }

    #[test]
    fn test_validate_detects_state_type() {
        let tokens = boruna_compiler::lexer::lex(COUNTER_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let result = AppValidator::validate(&program).unwrap();
        assert_eq!(result.state_type, Some("State".into()));
    }

    #[test]
    fn test_validate_detects_msg_type() {
        let tokens = boruna_compiler::lexer::lex(COUNTER_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let result = AppValidator::validate(&program).unwrap();
        assert_eq!(result.message_type, Some("Msg".into()));
    }

    #[test]
    fn test_validate_with_policies() {
        let tokens = boruna_compiler::lexer::lex(POLICY_APP).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let result = AppValidator::validate(&program).unwrap();
        assert!(result.has_policies);
    }

    // --- Runtime Tests ---

    #[test]
    fn test_runtime_init() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        // init() returns State { count: 0 }
        match runtime.state() {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(0));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    #[test]
    fn test_runtime_send_increment() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        let msg = AppMessage::new("increment", Value::Int(0));
        let (state, effects, ui) = runtime.send(msg).unwrap();

        match &state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(1));
            }
            other => panic!("expected Record, got {other}"),
        }
        assert!(effects.is_empty());
        assert!(ui.is_some());
    }

    #[test]
    fn test_runtime_multiple_messages() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        for _ in 0..5 {
            runtime
                .send(AppMessage::new("increment", Value::Int(0)))
                .unwrap();
        }
        runtime
            .send(AppMessage::new("decrement", Value::Int(0)))
            .unwrap();

        match runtime.state() {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(4));
            }
            other => panic!("expected Record, got {other}"),
        }
        assert_eq!(runtime.cycle(), 6);
    }

    #[test]
    fn test_runtime_with_effects() {
        let module = boruna_compiler::compile("test", EFFECT_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        let msg = AppMessage::new("fetch", Value::String(String::new()));
        let (_, effects, _) = runtime.send(msg).unwrap();

        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].kind, EffectKind::HttpRequest);
    }

    #[test]
    fn test_runtime_view() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        let ui = runtime.view().unwrap();
        // view returns UINode { tag: "text", text: "count" }
        match &ui {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("text".into()));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    // --- State Machine Tests ---

    #[test]
    fn test_state_machine_transition() {
        let mut sm = StateMachine::new(Value::Int(0));
        assert_eq!(sm.cycle(), 0);

        sm.transition(Value::Int(1));
        assert_eq!(sm.cycle(), 1);
        assert_eq!(sm.current(), &Value::Int(1));
    }

    #[test]
    fn test_state_machine_snapshot_restore() {
        let mut sm = StateMachine::new(Value::Int(42));
        let json = sm.snapshot();
        assert!(json.contains("42"));

        sm.transition(Value::Int(100));
        sm.restore(&json).unwrap();
        // After restore, it becomes a new transition
        assert_eq!(sm.current(), &Value::Int(42));
    }

    #[test]
    fn test_state_machine_diff() {
        let mut sm = StateMachine::new(Value::Record {
            type_id: 0,
            fields: vec![Value::Int(0), Value::String("hello".into())],
        });
        sm.transition(Value::Record {
            type_id: 0,
            fields: vec![Value::Int(1), Value::String("hello".into())],
        });

        let diffs = sm.diff_from_cycle(0);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].field_index, 0);
        assert_eq!(diffs[0].old_value, Value::Int(0));
        assert_eq!(diffs[0].new_value, Value::Int(1));
    }

    #[test]
    fn test_state_machine_rewind() {
        let mut sm = StateMachine::new(Value::Int(0));
        sm.transition(Value::Int(1));
        sm.transition(Value::Int(2));
        sm.transition(Value::Int(3));

        sm.rewind(1).unwrap();
        assert_eq!(sm.current(), &Value::Int(1));
    }

    #[test]
    fn test_state_machine_history() {
        let mut sm = StateMachine::new(Value::Int(0));
        sm.transition(Value::Int(1));
        sm.transition(Value::Int(2));

        assert_eq!(sm.history().len(), 3);
        assert_eq!(sm.history()[0].cycle, 0);
        assert_eq!(sm.history()[2].cycle, 2);
    }

    // --- Policy Tests ---

    #[test]
    fn test_policy_allow_all() {
        let policy = PolicySet::allow_all();
        assert!(!policy.capabilities.is_empty());
        assert_eq!(policy.max_effects_per_cycle, 0); // unlimited
    }

    #[test]
    fn test_policy_from_value() {
        let val = Value::Record {
            type_id: 0,
            fields: vec![
                Value::List(vec![Value::String("net.fetch".into())]),
                Value::Int(5),
                Value::Int(1000),
            ],
        };
        let policy = PolicySet::from_value(&val);
        assert_eq!(policy.capabilities, vec!["net.fetch"]);
        assert_eq!(policy.max_effects_per_cycle, 5);
        assert_eq!(policy.max_steps, 1000);
    }

    #[test]
    fn test_policy_check_batch_limit() {
        let policy = PolicySet {
            schema_version: 1,
            capabilities: vec!["net.fetch".into()],
            max_effects_per_cycle: 1,
            max_steps: 0,
        };
        let effects = vec![
            crate::effect::Effect {
                kind: EffectKind::HttpRequest,
                payload: Value::Unit,
                callback_tag: String::new(),
            },
            crate::effect::Effect {
                kind: EffectKind::HttpRequest,
                payload: Value::Unit,
                callback_tag: String::new(),
            },
        ];
        assert!(policy.check_batch(&effects).is_err());
    }

    // --- Policy Hardening Tests ---

    #[test]
    fn test_policy_deny_specific_capability() {
        // Policy only allows net.fetch — db.query should be denied.
        let policy = PolicySet {
            schema_version: 1,
            capabilities: vec!["net.fetch".into()],
            max_effects_per_cycle: 0,
            max_steps: 0,
        };
        let allowed = crate::effect::Effect {
            kind: EffectKind::HttpRequest,
            payload: Value::Unit,
            callback_tag: String::new(),
        };
        let denied = crate::effect::Effect {
            kind: EffectKind::DbQuery,
            payload: Value::Unit,
            callback_tag: String::new(),
        };
        assert!(policy.check_effect(&allowed).is_ok());
        assert!(policy.check_effect(&denied).is_err());
        let err = policy.check_effect(&denied).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("db.query"),
            "denial error should name the capability: {msg}"
        );
    }

    #[test]
    fn test_policy_empty_capabilities_allows_all() {
        // Empty capabilities list means no restrictions.
        let policy = PolicySet {
            schema_version: 1,
            capabilities: Vec::new(),
            max_effects_per_cycle: 0,
            max_steps: 0,
        };
        let effect = crate::effect::Effect {
            kind: EffectKind::FsWrite,
            payload: Value::Unit,
            callback_tag: String::new(),
        };
        assert!(policy.check_effect(&effect).is_ok());
    }

    #[test]
    fn test_policy_batch_limit_exact_boundary() {
        let policy = PolicySet {
            schema_version: 1,
            capabilities: Vec::new(),
            max_effects_per_cycle: 2,
            max_steps: 0,
        };
        let make_effect = || crate::effect::Effect {
            kind: EffectKind::HttpRequest,
            payload: Value::Unit,
            callback_tag: String::new(),
        };

        // Exactly at limit — should pass
        assert!(policy.check_batch(&[make_effect(), make_effect()]).is_ok());
        // Over limit — should fail
        assert!(policy
            .check_batch(&[make_effect(), make_effect(), make_effect()])
            .is_err());
    }

    #[test]
    fn test_policy_json_diagnostic() {
        let policy = PolicySet {
            schema_version: 1,
            capabilities: vec!["net.fetch".into(), "time.now".into()],
            max_effects_per_cycle: 5,
            max_steps: 100000,
        };
        let json = policy.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["capabilities"],
            serde_json::json!(["net.fetch", "time.now"])
        );
        assert_eq!(parsed["max_effects_per_cycle"], 5);
        assert_eq!(parsed["max_steps"], 100000);
    }

    #[test]
    fn test_policy_violation_json_diagnostic() {
        let err = crate::error::FrameworkError::PolicyViolation(
            "effect DbQuery requires capability 'db.query'".into(),
        );
        let json = crate::policy::error_to_json(&err);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["error"], "policy_violation");
        assert!(parsed["detail"].as_str().unwrap().contains("db.query"));
    }

    #[test]
    fn test_policy_from_value_with_record_list_literal() {
        // Test with Record{type_id: 0xFFFF} (how the language represents list literals)
        let val = Value::Record {
            type_id: 0,
            fields: vec![
                Value::Record {
                    type_id: 0xFFFF,
                    fields: vec![
                        Value::String("net.fetch".into()),
                        Value::String("fs.read".into()),
                    ],
                },
                Value::Int(3),
                Value::Int(500000),
            ],
        };
        let policy = PolicySet::from_value(&val);
        assert_eq!(policy.capabilities, vec!["net.fetch", "fs.read"]);
        assert_eq!(policy.max_effects_per_cycle, 3);
        assert_eq!(policy.max_steps, 500000);
    }

    #[test]
    fn test_policy_default_is_restrictive() {
        let policy = PolicySet::default();
        assert!(
            policy.capabilities.is_empty(),
            "default should have no capabilities"
        );
        assert_eq!(
            policy.max_effects_per_cycle, 0,
            "default should have unlimited effects"
        );
    }

    // --- Test Harness Tests ---

    #[test]
    fn test_harness_from_source() {
        let harness = TestHarness::from_source(COUNTER_APP).unwrap();
        match harness.state() {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(0));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    #[test]
    fn test_harness_simulate() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        let messages = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
        ];
        let final_state = harness.simulate(messages).unwrap();
        match &final_state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(3));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    #[test]
    fn test_harness_assert_state_field() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        harness
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        assert!(harness.assert_state_field(0, &Value::Int(1)).is_ok());
        assert!(harness.assert_state_field(0, &Value::Int(99)).is_err());
    }

    #[test]
    fn test_harness_assert_effects() {
        let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
        harness
            .send(AppMessage::new("fetch", Value::String(String::new())))
            .unwrap();
        assert!(harness.assert_effects(&["http_request"]).is_ok());
    }

    #[test]
    fn test_harness_snapshot_json() {
        let harness = TestHarness::from_source(COUNTER_APP).unwrap();
        let json = harness.snapshot();
        assert!(json.contains("Record") || json.contains("type_id") || json.contains("fields"));
    }

    #[test]
    fn test_harness_view() {
        let harness = TestHarness::from_source(COUNTER_APP).unwrap();
        let ui = harness.view().unwrap();
        match &ui {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("text".into()));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    #[test]
    fn test_harness_cycle_log() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        harness
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        harness
            .send(AppMessage::new("decrement", Value::Int(0)))
            .unwrap();
        assert_eq!(harness.cycle_log().len(), 2);
        assert_eq!(harness.cycle_log()[0].cycle, 1);
        assert_eq!(harness.cycle_log()[1].cycle, 2);
    }

    // --- UI Model Tests ---

    #[test]
    fn test_ui_value_to_tree() {
        let val = Value::Record {
            type_id: 0,
            fields: vec![
                Value::String("button".into()),
                Value::String("Click me".into()),
            ],
        };
        let tree = crate::ui::value_to_ui_tree(&val);
        assert_eq!(tree.tag, "button");
    }

    #[test]
    fn test_ui_tree_to_value() {
        let node =
            crate::ui::UINode::new("div").with_prop("class", Value::String("container".into()));
        let val = crate::ui::ui_tree_to_value(&node);
        match val {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("div".into()));
            }
            _ => panic!("expected Record"),
        }
    }

    // --- Replay Tests ---

    #[test]
    fn test_replay_verify() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        let messages = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
        ];
        // Run original
        for msg in &messages {
            harness.send(msg.clone()).unwrap();
        }

        // Replay and verify
        let replay_messages = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
        ];
        let result = harness.replay_verify(COUNTER_APP, replay_messages).unwrap();
        assert!(result, "replay should produce identical states");
    }

    #[test]
    fn test_replay_diverges() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        harness
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        harness
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();

        // Replay with different messages
        let replay_messages = vec![
            AppMessage::new("decrement", Value::Int(0)),
            AppMessage::new("decrement", Value::Int(0)),
        ];
        let result = harness.replay_verify(COUNTER_APP, replay_messages).unwrap();
        assert!(!result, "replay with different messages should diverge");
    }

    // --- End to End ---

    #[test]
    fn test_e2e_full_cycle() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        // Verify initial state
        match runtime.state() {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(0)),
            _ => panic!("bad init state"),
        }

        // Send messages
        let (s1, _, _) = runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        match &s1 {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(1)),
            _ => panic!("bad state after increment"),
        }

        let (s2, _, _) = runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        match &s2 {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(2)),
            _ => panic!("bad state after second increment"),
        }

        let (s3, _, _) = runtime
            .send(AppMessage::new("decrement", Value::Int(0)))
            .unwrap();
        match &s3 {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(1)),
            _ => panic!("bad state after decrement"),
        }

        // Verify cycle count
        assert_eq!(runtime.cycle(), 3);

        // Verify diff
        let diffs = runtime.diff_from(0);
        assert!(!diffs.is_empty());
    }

    #[test]
    fn test_e2e_with_policies() {
        let module = boruna_compiler::compile("test", POLICY_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        let policy = runtime.policy();
        assert_eq!(policy.capabilities, vec!["net.fetch", "time.now"]);
        assert_eq!(policy.max_effects_per_cycle, 10);
    }

    // --- Golden Determinism Tests ---
    // These ensure identical inputs always produce identical outputs.
    // If any golden test fails, a nondeterminism bug has been introduced.

    /// Compute a stable fingerprint of a cycle log for determinism checks.
    fn cycle_fingerprint(log: &[crate::runtime::CycleRecord]) -> String {
        let mut parts = Vec::new();
        for r in log {
            parts.push(format!(
                "c{}:msg={}:{},before={},after={},effects=[{}],ui={}",
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
                r.ui_tree
                    .as_ref()
                    .map(|v| format!("{v}"))
                    .unwrap_or("none".into()),
            ));
        }
        parts.join("|")
    }

    #[test]
    fn test_golden_counter_determinism() {
        let messages = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("decrement", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
        ];

        // Run twice with identical inputs
        let mut h1 = TestHarness::from_source(COUNTER_APP).unwrap();
        for msg in &messages {
            h1.send(msg.clone()).unwrap();
        }

        let mut h2 = TestHarness::from_source(COUNTER_APP).unwrap();
        for msg in &messages {
            h2.send(msg.clone()).unwrap();
        }

        // State must be identical
        assert_eq!(h1.state(), h2.state(), "final states must match");

        // Cycle logs must be identical
        let fp1 = cycle_fingerprint(h1.cycle_log());
        let fp2 = cycle_fingerprint(h2.cycle_log());
        assert_eq!(fp1, fp2, "cycle log fingerprints must match");

        // Snapshots must be identical
        assert_eq!(h1.snapshot(), h2.snapshot(), "JSON snapshots must match");
    }

    #[test]
    fn test_golden_todo_determinism() {
        let source = r#"
type State { total: Int, completed: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { total: 0, completed: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "add" {
        UpdateResult {
            state: State { total: state.total + 1, completed: state.completed },
            effects: [],
        }
    } else {
        if msg.tag == "complete" {
            let new_completed: Int = if state.completed < state.total {
                state.completed + 1
            } else {
                state.completed
            }
            UpdateResult {
                state: State { total: state.total, completed: new_completed },
                effects: [],
            }
        } else {
            UpdateResult { state: state, effects: [] }
        }
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "todo", text: "list" }
}
"#;

        let messages = vec![
            AppMessage::new("add", Value::Int(0)),
            AppMessage::new("add", Value::Int(0)),
            AppMessage::new("add", Value::Int(0)),
            AppMessage::new("complete", Value::Int(0)),
            AppMessage::new("complete", Value::Int(0)),
        ];

        let mut h1 = TestHarness::from_source(source).unwrap();
        let mut h2 = TestHarness::from_source(source).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        assert_eq!(h1.state(), h2.state());
        assert_eq!(
            cycle_fingerprint(h1.cycle_log()),
            cycle_fingerprint(h2.cycle_log())
        );
    }

    #[test]
    fn test_golden_effects_determinism() {
        let messages = vec![
            AppMessage::new("fetch", Value::String(String::new())),
            AppMessage::new("result", Value::String("data".into())),
            AppMessage::new("fetch", Value::String(String::new())),
        ];

        let mut h1 = TestHarness::from_source(EFFECT_APP).unwrap();
        let mut h2 = TestHarness::from_source(EFFECT_APP).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        let fp1 = cycle_fingerprint(h1.cycle_log());
        let fp2 = cycle_fingerprint(h2.cycle_log());
        assert_eq!(fp1, fp2, "effect cycle fingerprints must match");

        // Effect kinds must match cycle by cycle
        for (c1, c2) in h1.cycle_log().iter().zip(h2.cycle_log().iter()) {
            let k1: Vec<&str> = c1.effects.iter().map(|e| e.kind.as_str()).collect();
            let k2: Vec<&str> = c2.effects.iter().map(|e| e.kind.as_str()).collect();
            assert_eq!(k1, k2, "effect kinds must match at cycle {}", c1.cycle);
        }
    }

    #[test]
    fn test_golden_replay_equivalence_counter() {
        let messages = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("decrement", Value::Int(0)),
        ];

        let mut h1 = TestHarness::from_source(COUNTER_APP).unwrap();
        for msg in &messages {
            h1.send(msg.clone()).unwrap();
        }

        // Replay with same messages on fresh runtime
        let replay_msgs = vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("decrement", Value::Int(0)),
        ];
        let result = h1.replay_verify(COUNTER_APP, replay_msgs).unwrap();
        assert!(result, "replay must produce identical states");
    }

    #[test]
    fn test_golden_replay_equivalence_effects() {
        let messages = vec![
            AppMessage::new("fetch", Value::String(String::new())),
            AppMessage::new("result", Value::String("got_data".into())),
        ];

        let mut h1 = TestHarness::from_source(EFFECT_APP).unwrap();
        for msg in &messages {
            h1.send(msg.clone()).unwrap();
        }

        let replay_msgs = vec![
            AppMessage::new("fetch", Value::String(String::new())),
            AppMessage::new("result", Value::String("got_data".into())),
        ];
        let result = h1.replay_verify(EFFECT_APP, replay_msgs).unwrap();
        assert!(result, "effect app replay must be identical");
    }

    #[test]
    fn test_golden_snapshot_stability() {
        // Same messages must produce the exact same JSON snapshot string.
        let mut h1 = TestHarness::from_source(COUNTER_APP).unwrap();
        h1.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        h1.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        let snap1 = h1.snapshot();

        let mut h2 = TestHarness::from_source(COUNTER_APP).unwrap();
        h2.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        h2.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        let snap2 = h2.snapshot();

        assert_eq!(snap1, snap2, "JSON snapshots must be bitwise identical");
    }

    // --- Purity Enforcement Tests ---

    #[test]
    fn test_purity_update_denies_capabilities() {
        // Construct a module where update() has a CapCall instruction.
        // Even though the function declares the capability, the framework's
        // deny-all policy during update() must reject it.
        use boruna_bytecode::{Capability, Function, Module, Op};

        let mut module = Module::new("purity_test");

        // Type: State { count: Int }
        // init() -> State { count: 0 }
        let init_fn = Function {
            name: "init".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0), // Int(0)
                Op::MakeRecord(0, 1),
                Op::Ret,
            ],
            capabilities: Vec::new(),
            match_tables: Vec::new(),
        };

        // update(state, msg) -> UpdateResult
        // This update() tries to call a capability (TimeNow) — this must fail.
        let update_fn = Function {
            name: "update".into(),
            arity: 2,
            locals: 2,
            code: vec![
                // Try to call TimeNow capability
                Op::CapCall(Capability::TimeNow.id(), 0),
                // Then build result (never reached if enforcement works)
                Op::Pop,
                Op::LoadLocal(0),     // state
                Op::PushConst(1),     // empty list
                Op::MakeRecord(1, 2), // UpdateResult
                Op::Ret,
            ],
            capabilities: vec![Capability::TimeNow], // Function declares cap
            match_tables: Vec::new(),
        };

        // view(state) -> UINode
        let view_fn = Function {
            name: "view".into(),
            arity: 1,
            locals: 1,
            code: vec![
                Op::PushConst(2), // "text"
                Op::PushConst(3), // "count"
                Op::MakeRecord(2, 2),
                Op::Ret,
            ],
            capabilities: Vec::new(),
            match_tables: Vec::new(),
        };

        module.add_const(Value::Int(0));
        module.add_const(Value::List(vec![]));
        module.add_const(Value::String("text".into()));
        module.add_const(Value::String("count".into()));
        module.add_function(init_fn);
        module.add_function(update_fn);
        module.add_function(view_fn);
        module.entry = 0;

        let mut runtime = AppRuntime::new(module).unwrap();
        let result = runtime.send(AppMessage::new("test", Value::Int(0)));
        assert!(result.is_err(), "update() with CapCall must fail");
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("capability") || err.contains("denied"),
            "error should indicate capability denial: {err}"
        );
    }

    #[test]
    fn test_purity_view_denies_capabilities() {
        use boruna_bytecode::{Capability, Function, Module, Op};

        let mut module = Module::new("purity_view_test");

        let init_fn = Function {
            name: "init".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::PushConst(0), Op::MakeRecord(0, 1), Op::Ret],
            capabilities: Vec::new(),
            match_tables: Vec::new(),
        };

        // Pure update (no caps)
        let update_fn = Function {
            name: "update".into(),
            arity: 2,
            locals: 2,
            code: vec![
                Op::LoadLocal(0),
                Op::PushConst(1),
                Op::MakeRecord(1, 2),
                Op::Ret,
            ],
            capabilities: Vec::new(),
            match_tables: Vec::new(),
        };

        // view() tries to call TimeNow — must fail
        let view_fn = Function {
            name: "view".into(),
            arity: 1,
            locals: 1,
            code: vec![Op::CapCall(Capability::TimeNow.id(), 0), Op::Ret],
            capabilities: vec![Capability::TimeNow],
            match_tables: Vec::new(),
        };

        module.add_const(Value::Int(0));
        module.add_const(Value::List(vec![]));
        module.add_function(init_fn);
        module.add_function(update_fn);
        module.add_function(view_fn);
        module.entry = 0;

        let mut runtime = AppRuntime::new(module).unwrap();
        let result = runtime.send(AppMessage::new("test", Value::Int(0)));
        assert!(result.is_err(), "view() with CapCall must fail");
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("capability") || err.contains("denied"),
            "error should indicate capability denial: {err}"
        );
    }

    #[test]
    fn test_purity_init_allows_capabilities() {
        // init() is NOT pure — it may use capabilities for initial setup.
        // Verify this still works (the deny-all change must not break init).
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let runtime = AppRuntime::new(module);
        assert!(
            runtime.is_ok(),
            "init() should succeed without purity constraint"
        );
    }

    // --- Host Integration Tests ---
    // Simulates the full host contract: boot → render → event → state change → re-render.
    // No actual browser/Tauri needed — this tests the runtime contract that hosts depend on.

    #[test]
    fn test_host_integration_boot_render_event_cycle() {
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        // 1. Boot: initial state must be valid
        let init_state = runtime.state().clone();
        match &init_state {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(0)),
            _ => panic!("init state must be Record"),
        }

        // 2. Render: view must return a valid UI tree
        let ui = runtime.view().unwrap();
        match &ui {
            Value::Record { fields, .. } => {
                assert!(
                    matches!(&fields[0], Value::String(_)),
                    "UI tag must be String"
                );
            }
            _ => panic!("view must return Record"),
        }

        // 3. Event: simulate user click → dispatch message
        let (new_state, effects, ui_tree) = runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();

        // 4. Verify state change
        match &new_state {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(1)),
            _ => panic!("post-event state must be Record"),
        }

        // 5. Verify effects (counter has none)
        assert!(effects.is_empty());

        // 6. Verify UI tree was emitted
        assert!(ui_tree.is_some(), "UI tree must be emitted after update");
        let ui_after = ui_tree.unwrap();
        match &ui_after {
            Value::Record { fields, .. } => {
                assert!(matches!(&fields[0], Value::String(_)));
            }
            _ => panic!("emitted UI must be Record"),
        }

        // 7. Second event cycle
        let (state2, _, ui2) = runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        match &state2 {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::Int(2)),
            _ => panic!("second state must be Record"),
        }
        assert!(ui2.is_some());
    }

    #[test]
    fn test_host_integration_effects_round_trip() {
        let module = boruna_compiler::compile("test", EFFECT_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        // Dispatch a "fetch" event → should produce an http_request effect
        let (_, effects, _) = runtime
            .send(AppMessage::new("fetch", Value::String(String::new())))
            .unwrap();

        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].kind, EffectKind::HttpRequest);
        assert_eq!(effects[0].callback_tag, "fetched");

        // Host would execute the effect and deliver the result as a new message.
        // Simulate: "fetched" message with data
        let (state, effects2, _) = runtime
            .send(AppMessage::new(
                "fetched",
                Value::String("response_data".into()),
            ))
            .unwrap();

        // State should be updated with the data
        match &state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("response_data".into()));
            }
            _ => panic!("state should contain response data"),
        }
        assert!(
            effects2.is_empty(),
            "response should produce no new effects"
        );
    }

    #[test]
    fn test_host_integration_ui_tree_deterministic_ordering() {
        // Same state must produce identical UI trees — hosts depend on this for diffing.
        let module1 = boruna_compiler::compile("test1", COUNTER_APP).unwrap();
        let module2 = boruna_compiler::compile("test2", COUNTER_APP).unwrap();
        let mut rt1 = AppRuntime::new(module1).unwrap();
        let mut rt2 = AppRuntime::new(module2).unwrap();

        rt1.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        rt2.send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();

        let ui1 = rt1.view().unwrap();
        let ui2 = rt2.view().unwrap();
        assert_eq!(ui1, ui2, "UI trees must be identical for identical states");
    }

    #[test]
    fn test_host_integration_snapshot_for_persistence() {
        // Host may persist state between sessions via JSON snapshots.
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();

        // Snapshot for host persistence
        let json = runtime.snapshot();
        assert!(!json.is_empty());

        // Verify it's valid JSON
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&json);
        assert!(parsed.is_ok(), "snapshot must be valid JSON: {json}");
    }

    #[test]
    fn test_host_integration_cycle_log_for_devtools() {
        // Host devtools can inspect cycle log for debugging.
        let module = boruna_compiler::compile("test", COUNTER_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();

        runtime
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        runtime
            .send(AppMessage::new("decrement", Value::Int(0)))
            .unwrap();

        let log = runtime.cycle_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].message.tag, "increment");
        assert_eq!(log[1].message.tag, "decrement");
        assert_eq!(log[0].cycle, 1);
        assert_eq!(log[1].cycle, 2);

        // state_before of cycle 2 should equal state_after of cycle 1
        assert_eq!(log[1].state_before, log[0].state_after);
    }

    // --- API Surface Snapshot ---
    // These tests ensure the public API doesn't drift accidentally.
    // If a test fails, update FRAMEWORK_API.md and the snapshot.

    #[test]
    fn test_api_snapshot_crate_reexports() {
        // Verify crate root re-exports exist by constructing/using them.
        // FrameworkError
        let _err: crate::FrameworkError = crate::FrameworkError::Validation("test".into());
        // AppValidator
        let _valid = crate::AppValidator::is_valid_app;
        // PolicySet
        let _ps = crate::PolicySet::allow_all();
        // TestHarness (constructor needs source, just check type exists)
        fn _check_harness(_h: &crate::TestHarness) {}
        // AppRuntime
        fn _check_runtime(_r: &crate::AppRuntime) {}
    }

    #[test]
    fn test_api_snapshot_effect_kinds() {
        // All 8 effect kinds must exist and round-trip through as_str/from_str.
        let kinds = [
            "http_request",
            "db_query",
            "fs_read",
            "fs_write",
            "timer",
            "random",
            "spawn_actor",
            "emit_ui",
        ];
        for kind_str in &kinds {
            let kind = EffectKind::parse_str(kind_str);
            assert!(
                kind.is_some(),
                "EffectKind::parse_str({kind_str}) should exist"
            );
            assert_eq!(kind.unwrap().as_str(), *kind_str);
        }
        assert_eq!(kinds.len(), 8, "exactly 8 effect kinds");
    }

    #[test]
    fn test_api_snapshot_policy_set_fields() {
        let ps = PolicySet::allow_all();
        // Public fields must exist
        let _caps: &Vec<String> = &ps.capabilities;
        let _max_eff: u64 = ps.max_effects_per_cycle;
        let _max_steps: u64 = ps.max_steps;
    }

    #[test]
    fn test_api_snapshot_cycle_record_fields() {
        let mut harness = TestHarness::from_source(COUNTER_APP).unwrap();
        harness
            .send(AppMessage::new("increment", Value::Int(0)))
            .unwrap();
        let record = &harness.cycle_log()[0];
        // All public fields must be accessible
        let _cycle: u64 = record.cycle;
        let _msg: &AppMessage = &record.message;
        let _before: &Value = &record.state_before;
        let _after: &Value = &record.state_after;
        let _effects: &Vec<crate::effect::Effect> = &record.effects;
        let _ui: &Option<Value> = &record.ui_tree;
    }

    // ================================================================
    // Dogfood App Tests — Admin CRUD, Notifications, Offline Sync Todo
    // ================================================================

    const ADMIN_CRUD_APP: &str = include_str!("../../../examples/admin_crud/admin_crud_app.ax");
    const NOTIFICATION_APP: &str =
        include_str!("../../../examples/realtime_notifications/notification_app.ax");
    const SYNC_TODO_APP: &str =
        include_str!("../../../examples/offline_sync_todo/sync_todo_app.ax");

    // --- Admin CRUD: Golden Determinism ---

    #[test]
    fn test_dogfood_admin_crud_determinism() {
        let messages = vec![
            AppMessage::new("create_user", Value::String(String::new())),
            AppMessage::new("user_created", Value::String("ok".into())),
            AppMessage::new("list_users", Value::String(String::new())),
            AppMessage::new("users_listed", Value::String("1 user".into())),
            AppMessage::new("delete_user", Value::String(String::new())),
            AppMessage::new("user_deleted", Value::String("ok".into())),
        ];

        let mut h1 = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();
        let mut h2 = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        assert_eq!(h1.state(), h2.state(), "admin CRUD states must match");
        assert_eq!(
            cycle_fingerprint(h1.cycle_log()),
            cycle_fingerprint(h2.cycle_log()),
            "admin CRUD cycle fingerprints must match"
        );
        assert_eq!(
            h1.snapshot(),
            h2.snapshot(),
            "admin CRUD snapshots must match"
        );
    }

    // --- Admin CRUD: Replay Equivalence ---

    #[test]
    fn test_dogfood_admin_crud_replay() {
        let messages = vec![
            AppMessage::new("create_user", Value::String(String::new())),
            AppMessage::new("user_created", Value::String("ok".into())),
            AppMessage::new("edit_user", Value::String(String::new())),
            AppMessage::new("user_updated", Value::String("ok".into())),
        ];

        let mut h = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();
        for msg in &messages {
            h.send(msg.clone()).unwrap();
        }

        let result = h.replay_verify(ADMIN_CRUD_APP, messages).unwrap();
        assert!(result, "admin CRUD replay must be identical");
    }

    // --- Admin CRUD: Authorization Policy ---

    #[test]
    fn test_dogfood_admin_crud_authorization() {
        let mut h = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();

        // Default role is admin — create should work
        let (state, effects) = h
            .send(AppMessage::new("create_user", Value::String(String::new())))
            .unwrap();
        match &state {
            Value::Record { fields, .. } => {
                // mode field (index 0) should be "creating"
                assert_eq!(fields[0], Value::String("creating".into()));
            }
            _ => panic!("expected Record state"),
        }
        assert_eq!(effects.len(), 1, "create should produce db effect");
        assert_eq!(effects[0].kind, EffectKind::DbQuery);

        // Accept callback
        h.send(AppMessage::new("user_created", Value::String("ok".into())))
            .unwrap();

        // Switch to viewer role
        h.send(AppMessage::new("set_role", Value::String("viewer".into())))
            .unwrap();

        // Now create should be denied
        let (state2, effects2) = h
            .send(AppMessage::new("create_user", Value::String(String::new())))
            .unwrap();
        match &state2 {
            Value::Record { fields, .. } => {
                // status field (index 5) should be "error"
                assert_eq!(fields[5], Value::String("error".into()));
                // status_detail (index 6) should mention unauthorized
                if let Value::String(detail) = &fields[6] {
                    assert!(
                        detail.contains("unauthorized"),
                        "should say unauthorized: {detail}"
                    );
                }
            }
            _ => panic!("expected Record state"),
        }
        assert!(
            effects2.is_empty(),
            "denied action should produce no effects"
        );
    }

    // --- Admin CRUD: Delete authorization ---

    #[test]
    fn test_dogfood_admin_crud_delete_auth() {
        let mut h = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();

        // Switch to editor role — editor can create but NOT delete
        h.send(AppMessage::new("set_role", Value::String("editor".into())))
            .unwrap();

        // Delete should be denied for non-admin
        let (state, effects) = h
            .send(AppMessage::new("delete_user", Value::String(String::new())))
            .unwrap();
        match &state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[5], Value::String("error".into()));
                if let Value::String(detail) = &fields[6] {
                    assert!(
                        detail.contains("only admin"),
                        "should say only admin: {detail}"
                    );
                }
            }
            _ => panic!("expected Record state"),
        }
        assert!(effects.is_empty());
    }

    // --- Admin CRUD: Policy enforcement (db capability) ---

    #[test]
    fn test_dogfood_admin_crud_policy() {
        let module = boruna_compiler::compile("test", ADMIN_CRUD_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        let policy = runtime.policy();
        assert_eq!(policy.capabilities, vec!["db.query"]);
        assert_eq!(policy.max_effects_per_cycle, 5);
    }

    // --- Admin CRUD: CRUD flow state transitions ---

    #[test]
    fn test_dogfood_admin_crud_full_flow() {
        let mut h = TestHarness::from_source(ADMIN_CRUD_APP).unwrap();

        // Create 3 users
        for _ in 0..3 {
            h.send(AppMessage::new("create_user", Value::String(String::new())))
                .unwrap();
            h.send(AppMessage::new("user_created", Value::String("ok".into())))
                .unwrap();
        }

        // user_count (field 7) should be 3
        h.assert_state_field(7, &Value::Int(3)).unwrap();

        // Search
        let (_, effects) = h
            .send(AppMessage::new("search", Value::String("alice".into())))
            .unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].kind, EffectKind::DbQuery);

        h.send(AppMessage::new(
            "search_results",
            Value::String("1 result".into()),
        ))
        .unwrap();

        // Delete one
        h.send(AppMessage::new("delete_user", Value::String(String::new())))
            .unwrap();
        h.send(AppMessage::new("user_deleted", Value::String("ok".into())))
            .unwrap();

        // user_count should be 2
        h.assert_state_field(7, &Value::Int(2)).unwrap();
    }

    // --- Notification App: Golden Determinism ---

    #[test]
    fn test_dogfood_notification_determinism() {
        let messages = vec![
            AppMessage::new("subscribe", Value::String(String::new())),
            AppMessage::new("poll_tick", Value::String(String::new())),
            AppMessage::new("events_received", Value::String("alert1".into())),
            AppMessage::new("events_received", Value::String("alert2".into())),
            AppMessage::new("poll_tick", Value::String(String::new())),
            AppMessage::new("events_received", Value::String("alert3".into())),
            AppMessage::new("unsubscribe", Value::String(String::new())),
        ];

        let mut h1 = TestHarness::from_source(NOTIFICATION_APP).unwrap();
        let mut h2 = TestHarness::from_source(NOTIFICATION_APP).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        assert_eq!(h1.state(), h2.state());
        assert_eq!(
            cycle_fingerprint(h1.cycle_log()),
            cycle_fingerprint(h2.cycle_log())
        );
        assert_eq!(h1.snapshot(), h2.snapshot());
    }

    // --- Notification App: Replay ---

    #[test]
    fn test_dogfood_notification_replay() {
        let messages = vec![
            AppMessage::new("subscribe", Value::String(String::new())),
            AppMessage::new("poll_tick", Value::String(String::new())),
            AppMessage::new("events_received", Value::String("event1".into())),
            AppMessage::new("unsubscribe", Value::String(String::new())),
        ];

        let mut h = TestHarness::from_source(NOTIFICATION_APP).unwrap();
        for msg in &messages {
            h.send(msg.clone()).unwrap();
        }

        let result = h.replay_verify(NOTIFICATION_APP, messages).unwrap();
        assert!(result, "notification replay must be identical");
    }

    // --- Notification App: Message Ordering ---

    #[test]
    fn test_dogfood_notification_message_ordering() {
        let mut h = TestHarness::from_source(NOTIFICATION_APP).unwrap();

        h.send(AppMessage::new("subscribe", Value::String(String::new())))
            .unwrap();
        h.send(AppMessage::new("poll_tick", Value::String(String::new())))
            .unwrap();

        // Deliver 5 events in order
        for i in 1..6 {
            let data = format!("event_{i}");
            h.send(AppMessage::new("events_received", Value::String(data)))
                .unwrap();
        }

        // Verify sequence numbers are monotonic
        match h.state() {
            Value::Record { fields, .. } => {
                // events_received (field 1) should be 5
                assert_eq!(fields[1], Value::Int(5));
                // last_event_seq (field 2) should be 5
                assert_eq!(fields[2], Value::Int(5));
                // last_event_data (field 4) should be "event_5"
                assert_eq!(fields[4], Value::String("event_5".into()));
            }
            _ => panic!("expected Record state"),
        }

        // Verify cycle log shows correct ordering
        let log = h.cycle_log();
        for (i, record) in log.iter().skip(2).enumerate() {
            // Events start at cycle 3 (after subscribe + poll_tick)
            assert_eq!(record.message.tag, "events_received");
            let expected_data = format!("event_{}", i + 1);
            assert_eq!(record.message.payload, Value::String(expected_data));
        }
    }

    // --- Notification App: Rate Limiting ---

    #[test]
    fn test_dogfood_notification_rate_limiting() {
        let mut h = TestHarness::from_source(NOTIFICATION_APP).unwrap();

        h.send(AppMessage::new("subscribe", Value::String(String::new())))
            .unwrap();
        h.send(AppMessage::new("poll_tick", Value::String(String::new())))
            .unwrap();

        // Send 11 events (rate limit is 10)
        for i in 0..11 {
            let data = format!("event_{i}");
            h.send(AppMessage::new("events_received", Value::String(data)))
                .unwrap();
        }

        match h.state() {
            Value::Record { fields, .. } => {
                // events_received (field 1) should be 10 (rate limited)
                assert_eq!(fields[1], Value::Int(10));
                // rate_limited_count (field 7) should be 1
                assert_eq!(fields[7], Value::Int(1));
            }
            _ => panic!("expected Record state"),
        }
    }

    // --- Notification App: Policy ---

    #[test]
    fn test_dogfood_notification_policy() {
        let module = boruna_compiler::compile("test", NOTIFICATION_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        let policy = runtime.policy();
        assert_eq!(policy.capabilities, vec!["net.fetch", "time.now"]);
        assert_eq!(policy.max_effects_per_cycle, 3);
    }

    // --- Notification App: Subscription lifecycle ---

    #[test]
    fn test_dogfood_notification_subscription_lifecycle() {
        let mut h = TestHarness::from_source(NOTIFICATION_APP).unwrap();

        // Initially not subscribed
        match h.state() {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(0)); // subscribed = 0
            }
            _ => panic!("expected Record state"),
        }

        // Subscribe
        let (_, effects) = h
            .send(AppMessage::new("subscribe", Value::String(String::new())))
            .unwrap();
        assert_eq!(effects.len(), 1, "subscribe should start timer");

        // Double subscribe is no-op
        let (_, effects2) = h
            .send(AppMessage::new("subscribe", Value::String(String::new())))
            .unwrap();
        assert!(effects2.is_empty(), "double subscribe should be no-op");

        // Unsubscribe
        h.send(AppMessage::new("unsubscribe", Value::String(String::new())))
            .unwrap();
        match h.state() {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(0)); // subscribed = 0
                assert_eq!(fields[8], Value::Int(0)); // poll_active = 0
            }
            _ => panic!("expected Record state"),
        }

        // Poll tick after unsubscribe should not re-activate
        let (_, effects3) = h
            .send(AppMessage::new("poll_tick", Value::String(String::new())))
            .unwrap();
        assert!(
            effects3.is_empty(),
            "poll after unsubscribe should produce no effects"
        );
    }

    // --- Sync Todo: Golden Determinism ---

    #[test]
    fn test_dogfood_sync_todo_determinism() {
        let messages = vec![
            AppMessage::new("add_todo", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("ok".into())),
            AppMessage::new("go_offline", Value::String(String::new())),
            AppMessage::new("add_todo", Value::String(String::new())),
            AppMessage::new("add_todo", Value::String(String::new())),
            AppMessage::new("go_online", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("conflict".into())),
            AppMessage::new("conflict_resolved", Value::String("ok".into())),
        ];

        let mut h1 = TestHarness::from_source(SYNC_TODO_APP).unwrap();
        let mut h2 = TestHarness::from_source(SYNC_TODO_APP).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        assert_eq!(h1.state(), h2.state());
        assert_eq!(
            cycle_fingerprint(h1.cycle_log()),
            cycle_fingerprint(h2.cycle_log())
        );
        assert_eq!(h1.snapshot(), h2.snapshot());
    }

    // --- Sync Todo: Replay ---

    #[test]
    fn test_dogfood_sync_todo_replay() {
        let messages = vec![
            AppMessage::new("add_todo", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("ok".into())),
            AppMessage::new("complete_todo", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("ok".into())),
        ];

        let mut h = TestHarness::from_source(SYNC_TODO_APP).unwrap();
        for msg in &messages {
            h.send(msg.clone()).unwrap();
        }

        let result = h.replay_verify(SYNC_TODO_APP, messages).unwrap();
        assert!(result, "sync todo replay must be identical");
    }

    // --- Sync Todo: Conflict Resolution ---

    #[test]
    fn test_dogfood_sync_todo_conflict_resolution() {
        let mut h = TestHarness::from_source(SYNC_TODO_APP).unwrap();

        // Add todo online
        h.send(AppMessage::new("add_todo", Value::String(String::new())))
            .unwrap();

        // Server reports conflict
        let (state, effects) = h
            .send(AppMessage::new(
                "sync_response",
                Value::String("conflict".into()),
            ))
            .unwrap();

        match &state {
            Value::Record { fields, .. } => {
                // sync_status (field 6) should be "resolving_conflict"
                assert_eq!(fields[6], Value::String("resolving_conflict".into()));
                // conflicts_detected (field 9) should be 1
                assert_eq!(fields[9], Value::Int(1));
                // last_conflict_resolution (field 11) should be "local_wins"
                assert_eq!(fields[11], Value::String("local_wins".into()));
            }
            _ => panic!("expected Record state"),
        }
        // Should have sent a force_sync effect
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].kind, EffectKind::HttpRequest);
        assert_eq!(effects[0].callback_tag, "conflict_resolved");

        // Resolve conflict
        h.send(AppMessage::new(
            "conflict_resolved",
            Value::String("ok".into()),
        ))
        .unwrap();

        match h.state() {
            Value::Record { fields, .. } => {
                // conflicts_resolved (field 10) should be 1
                assert_eq!(fields[10], Value::Int(1));
                // sync_status should be "synced"
                assert_eq!(fields[6], Value::String("synced".into()));
                // versions should be aligned
                assert_eq!(
                    fields[7], fields[8],
                    "local and remote versions should match"
                );
            }
            _ => panic!("expected Record state"),
        }
    }

    // --- Sync Todo: Offline Queue ---

    #[test]
    fn test_dogfood_sync_todo_offline_queue() {
        let mut h = TestHarness::from_source(SYNC_TODO_APP).unwrap();

        // Go offline
        h.send(AppMessage::new("go_offline", Value::String(String::new())))
            .unwrap();

        // Add 3 items offline — should queue edits, no effects
        for _ in 0..3 {
            let (_, effects) = h
                .send(AppMessage::new("add_todo", Value::String(String::new())))
                .unwrap();
            assert!(effects.is_empty(), "offline add should produce no effects");
        }

        match h.state() {
            Value::Record { fields, .. } => {
                // total_items (field 0) should be 3
                assert_eq!(fields[0], Value::Int(3));
                // pending_edits (field 4) should be 3
                assert_eq!(fields[4], Value::Int(3));
                // sync_status (field 6) should be "offline_queued"
                assert_eq!(fields[6], Value::String("offline_queued".into()));
            }
            _ => panic!("expected Record state"),
        }

        // Go online — should flush pending
        let (_, effects) = h
            .send(AppMessage::new("go_online", Value::String(String::new())))
            .unwrap();
        assert_eq!(effects.len(), 1, "go_online should trigger bulk sync");
        assert_eq!(effects[0].kind, EffectKind::HttpRequest);
    }

    // --- Sync Todo: Policy (network budget) ---

    #[test]
    fn test_dogfood_sync_todo_policy() {
        let module = boruna_compiler::compile("test", SYNC_TODO_APP).unwrap();
        let runtime = AppRuntime::new(module).unwrap();
        let policy = runtime.policy();
        assert_eq!(policy.capabilities, vec!["net.fetch"]);
        assert_eq!(policy.max_effects_per_cycle, 2);
    }

    // --- Sync Todo: Snapshot stability ---

    #[test]
    fn test_dogfood_sync_todo_snapshot_stability() {
        let messages = vec![
            AppMessage::new("add_todo", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("ok".into())),
            AppMessage::new("complete_todo", Value::String(String::new())),
            AppMessage::new("sync_response", Value::String("ok".into())),
        ];

        let mut h1 = TestHarness::from_source(SYNC_TODO_APP).unwrap();
        let mut h2 = TestHarness::from_source(SYNC_TODO_APP).unwrap();

        for msg in &messages {
            h1.send(msg.clone()).unwrap();
            h2.send(msg.clone()).unwrap();
        }

        assert_eq!(
            h1.snapshot(),
            h2.snapshot(),
            "sync todo snapshots must be bitwise identical"
        );
    }

    // --- Cross-app: Trace hash stability ---

    #[test]
    fn test_dogfood_trace_hash_stability() {
        // Verify trace hashes are stable across runs for all 3 dogfood apps
        let apps: Vec<(&str, Vec<AppMessage>)> = vec![
            (
                ADMIN_CRUD_APP,
                vec![
                    AppMessage::new("create_user", Value::String(String::new())),
                    AppMessage::new("user_created", Value::String("ok".into())),
                ],
            ),
            (
                NOTIFICATION_APP,
                vec![
                    AppMessage::new("subscribe", Value::String(String::new())),
                    AppMessage::new("poll_tick", Value::String(String::new())),
                ],
            ),
            (
                SYNC_TODO_APP,
                vec![
                    AppMessage::new("add_todo", Value::String(String::new())),
                    AppMessage::new("sync_response", Value::String("ok".into())),
                ],
            ),
        ];

        for (source, messages) in &apps {
            let mut h1 = TestHarness::from_source(source).unwrap();
            let mut h2 = TestHarness::from_source(source).unwrap();

            for msg in messages {
                h1.send(msg.clone()).unwrap();
                h2.send(msg.clone()).unwrap();
            }

            let fp1 = cycle_fingerprint(h1.cycle_log());
            let fp2 = cycle_fingerprint(h2.cycle_log());
            assert_eq!(fp1, fp2, "trace fingerprints must match across runs");
        }
    }

    // === Effect Executor Tests ===

    use crate::effect::Effect;
    use crate::executor::{EffectExecutor, HostEffectExecutor, MockEffectExecutor};

    /// App that returns multiple effects of different kinds.
    const MULTI_EFFECT_APP: &str = r#"
type State { status: String }
type Msg { tag: String, payload: String }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { status: "idle" }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "do_stuff" {
        UpdateResult {
            state: State { status: "busy" },
            effects: [
                Effect { kind: "http_request", payload: "https://api.example.com", callback_tag: "http_done" },
                Effect { kind: "timer", payload: "", callback_tag: "time_done" },
                Effect { kind: "db_query", payload: "SELECT 1", callback_tag: "db_done" },
            ],
        }
    } else {
        UpdateResult {
            state: State { status: msg.payload },
            effects: [],
        }
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: state.status }
}
"#;

    #[test]
    fn test_mock_executor_delivers_callbacks() {
        let mut executor = MockEffectExecutor::new();
        executor.set_response("fetched", Value::String("response_data".into()));

        let effects = vec![Effect {
            kind: EffectKind::HttpRequest,
            payload: Value::String("https://example.com".into()),
            callback_tag: "fetched".into(),
        }];

        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "fetched");
        assert_eq!(messages[0].payload, Value::String("response_data".into()));
    }

    #[test]
    fn test_host_executor_http_request() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::HttpRequest,
            payload: Value::String("https://example.com".into()),
            callback_tag: "result".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "result");
        // MockHandler returns JSON string for NetFetch
        match &messages[0].payload {
            Value::String(s) => assert!(s.contains("mock")),
            other => panic!("expected String, got {other}"),
        }
    }

    #[test]
    fn test_host_executor_db_query() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::DbQuery,
            payload: Value::String("SELECT 1".into()),
            callback_tag: "db_result".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "db_result");
        // MockHandler returns empty List for DbQuery
        assert_eq!(messages[0].payload, Value::List(vec![]));
    }

    #[test]
    fn test_host_executor_timer() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::Timer,
            payload: Value::Unit,
            callback_tag: "tick".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "tick");
        assert_eq!(messages[0].payload, Value::Int(1700000000));
    }

    #[test]
    fn test_host_executor_random() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::Random,
            payload: Value::Unit,
            callback_tag: "rng".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "rng");
        assert_eq!(messages[0].payload, Value::Float(0.42));
    }

    #[test]
    fn test_host_executor_fs_read() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::FsRead,
            payload: Value::String("/tmp/test.txt".into()),
            callback_tag: "file_read".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "file_read");
        match &messages[0].payload {
            Value::String(s) => assert!(s.contains("mock file content")),
            other => panic!("expected String, got {other}"),
        }
    }

    #[test]
    fn test_host_executor_fs_write() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::FsWrite,
            payload: Value::String("/tmp/out.txt".into()),
            callback_tag: "file_written".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "file_written");
        assert_eq!(messages[0].payload, Value::Bool(true));
    }

    #[test]
    fn test_executor_emit_ui_no_callback() {
        let mut executor = MockEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::EmitUi,
            payload: Value::String("ui_tree".into()),
            callback_tag: "should_not_fire".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert!(messages.is_empty(), "EmitUi should not produce callbacks");
    }

    #[test]
    fn test_executor_preserves_effect_order() {
        let mut executor = MockEffectExecutor::new();
        executor.set_response("http_done", Value::String("http_result".into()));
        executor.set_response("time_done", Value::Int(12345));
        executor.set_response("db_done", Value::String("db_result".into()));

        let effects = vec![
            Effect {
                kind: EffectKind::HttpRequest,
                payload: Value::String("url".into()),
                callback_tag: "http_done".into(),
            },
            Effect {
                kind: EffectKind::Timer,
                payload: Value::Unit,
                callback_tag: "time_done".into(),
            },
            Effect {
                kind: EffectKind::DbQuery,
                payload: Value::String("query".into()),
                callback_tag: "db_done".into(),
            },
        ];

        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].tag, "http_done");
        assert_eq!(messages[1].tag, "time_done");
        assert_eq!(messages[2].tag, "db_done");
    }

    #[test]
    fn test_executor_spawn_actor_goes_through_gateway() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![Effect {
            kind: EffectKind::SpawnActor,
            payload: Value::String("child".into()),
            callback_tag: "spawned".into(),
        }];
        let messages = executor.execute(effects).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tag, "spawned");
        // SpawnActor now maps to Capability::ActorSpawn and goes through the gateway
        assert_eq!(messages[0].payload, Value::Unit);
    }

    #[test]
    fn test_full_effect_round_trip() {
        let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
        let mut executor = MockEffectExecutor::new();
        executor.set_response("fetched", Value::String("server_data".into()));

        // Step 1: Send "fetch" → gets effects
        let (_, callbacks) = harness
            .send_with_effects(
                AppMessage::new("fetch", Value::String(String::new())),
                &mut executor,
            )
            .unwrap();

        assert_eq!(callbacks.len(), 1);
        assert_eq!(callbacks[0].tag, "fetched");

        // Step 2: Feed callback back → state updates
        let (state, _) = harness.send(callbacks[0].clone()).unwrap();
        match &state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("server_data".into()));
            }
            other => panic!("expected Record, got {other}"),
        }
    }

    #[test]
    fn test_runtime_send_with_executor() {
        let module = boruna_compiler::compile("test", MULTI_EFFECT_APP).unwrap();
        let mut runtime = AppRuntime::new(module).unwrap();
        let mut executor = MockEffectExecutor::new();
        executor.set_response("http_done", Value::String("ok".into()));
        executor.set_response("time_done", Value::Int(100));
        executor.set_response("db_done", Value::String("rows".into()));

        let (state, callbacks, _ui) = runtime
            .send_with_executor(
                AppMessage::new("do_stuff", Value::String(String::new())),
                &mut executor,
            )
            .unwrap();

        // State should be "busy"
        match &state {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::String("busy".into()));
            }
            other => panic!("expected Record, got {other}"),
        }

        // Should have 3 callbacks in order
        assert_eq!(callbacks.len(), 3);
        assert_eq!(callbacks[0].tag, "http_done");
        assert_eq!(callbacks[1].tag, "time_done");
        assert_eq!(callbacks[2].tag, "db_done");
    }

    #[test]
    fn test_host_executor_event_log_records_calls() {
        let mut executor = HostEffectExecutor::new();
        let effects = vec![
            Effect {
                kind: EffectKind::HttpRequest,
                payload: Value::String("url".into()),
                callback_tag: "done".into(),
            },
            Effect {
                kind: EffectKind::Timer,
                payload: Value::Unit,
                callback_tag: "tick".into(),
            },
        ];
        executor.execute(effects).unwrap();

        // Should have logged 2 CapCall + 2 CapResult = 4 events
        assert_eq!(executor.event_log().events().len(), 4);
    }

    // === Determinism Invariant Tests ===

    #[test]
    fn test_invariant_effect_execution_order_deterministic() {
        // INV-1: Effect execution order is deterministic across runs
        for _ in 0..10 {
            let mut harness = TestHarness::from_source(MULTI_EFFECT_APP).unwrap();
            let mut executor = MockEffectExecutor::new();
            executor.set_response("http_done", Value::String("ok".into()));
            executor.set_response("time_done", Value::Int(100));
            executor.set_response("db_done", Value::String("rows".into()));

            let (_, callbacks) = harness
                .send_with_effects(
                    AppMessage::new("do_stuff", Value::String(String::new())),
                    &mut executor,
                )
                .unwrap();

            assert_eq!(callbacks.len(), 3);
            assert_eq!(callbacks[0].tag, "http_done");
            assert_eq!(callbacks[1].tag, "time_done");
            assert_eq!(callbacks[2].tag, "db_done");
        }
    }

    #[test]
    fn test_invariant_purity_only_effects_channel_data() {
        // INV-2: Effects are the ONLY channel for external data into update()
        // Two runs with different effect responses must produce different states
        let mut h1 = TestHarness::from_source(EFFECT_APP).unwrap();
        let mut h2 = TestHarness::from_source(EFFECT_APP).unwrap();

        // Both start with "fetch" → identical effects
        h1.send(AppMessage::new("fetch", Value::String(String::new())))
            .unwrap();
        h2.send(AppMessage::new("fetch", Value::String(String::new())))
            .unwrap();

        // Feed different callback data
        h1.send(AppMessage::new("fetched", Value::String("data_A".into())))
            .unwrap();
        h2.send(AppMessage::new("fetched", Value::String("data_B".into())))
            .unwrap();

        // States must differ (proving effects are the channel)
        assert_ne!(h1.state(), h2.state());
        // But each run's state must be correct
        match h1.state() {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::String("data_A".into())),
            _ => panic!("expected Record"),
        }
        match h2.state() {
            Value::Record { fields, .. } => assert_eq!(fields[0], Value::String("data_B".into())),
            _ => panic!("expected Record"),
        }
    }

    #[test]
    fn test_invariant_update_effect_list_deterministic() {
        // INV-3: Same (state, msg) → identical effect list, every time
        let mut reference_effects: Option<Vec<String>> = None;

        for _ in 0..20 {
            let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
            let (_, effects) = harness
                .send(AppMessage::new("fetch", Value::String(String::new())))
                .unwrap();

            let effect_strs: Vec<String> = effects
                .iter()
                .map(|e| format!("{}:{}:{}", e.kind.as_str(), e.payload, e.callback_tag))
                .collect();

            match &reference_effects {
                None => reference_effects = Some(effect_strs),
                Some(ref_effects) => {
                    assert_eq!(
                        &effect_strs, ref_effects,
                        "effects must be identical across runs"
                    );
                }
            }
        }
    }

    #[test]
    fn test_invariant_failed_effect_deterministic_error() {
        // INV-4: Failed effects produce identical error messages across runs
        let mut results = Vec::new();

        for _ in 0..10 {
            let mut executor = HostEffectExecutor::new();
            let effects = vec![Effect {
                kind: EffectKind::SpawnActor,
                payload: Value::String("child_actor".into()),
                callback_tag: "spawn_result".into(),
            }];
            let messages = executor.execute(effects).unwrap();
            assert_eq!(messages.len(), 1);
            results.push(messages[0].payload.clone());
        }

        // All error messages must be identical
        for result in &results {
            assert_eq!(
                result, &results[0],
                "error messages must be identical across runs"
            );
        }
    }

    #[test]
    fn test_invariant_cycle_fingerprint_stable_with_effects() {
        // INV-5: Cycle log fingerprint is stable across runs, even with effect callbacks
        let run = |executor: &mut MockEffectExecutor| -> String {
            let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
            // fetch → effects → callback → state update
            let (_, callbacks) = harness
                .send_with_effects(
                    AppMessage::new("fetch", Value::String(String::new())),
                    executor,
                )
                .unwrap();
            for cb in callbacks {
                harness.send(cb).unwrap();
            }
            cycle_fingerprint(harness.cycle_log())
        };

        let mut exec1 = MockEffectExecutor::new();
        exec1.set_response("fetched", Value::String("result".into()));
        let mut exec2 = MockEffectExecutor::new();
        exec2.set_response("fetched", Value::String("result".into()));

        let fp1 = run(&mut exec1);
        let fp2 = run(&mut exec2);
        assert_eq!(fp1, fp2, "fingerprints must be identical");
    }

    #[test]
    fn test_invariant_snapshot_bitwise_identical_with_callbacks() {
        // INV-6: State snapshot JSON is bitwise identical after identical message sequences
        let run = |executor: &mut MockEffectExecutor| -> String {
            let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
            let (_, callbacks) = harness
                .send_with_effects(
                    AppMessage::new("fetch", Value::String(String::new())),
                    executor,
                )
                .unwrap();
            for cb in callbacks {
                harness.send(cb).unwrap();
            }
            harness.snapshot()
        };

        let mut exec1 = MockEffectExecutor::new();
        exec1.set_response("fetched", Value::String("data_123".into()));
        let mut exec2 = MockEffectExecutor::new();
        exec2.set_response("fetched", Value::String("data_123".into()));

        let snap1 = run(&mut exec1);
        let snap2 = run(&mut exec2);
        assert_eq!(snap1, snap2, "snapshots must be bitwise identical");
    }

    #[test]
    fn test_invariant_replay_with_effects_matches_original() {
        // INV-7: Replay of effect-producing app matches original
        let mut harness = TestHarness::from_source(EFFECT_APP).unwrap();
        let mut executor = MockEffectExecutor::new();
        executor.set_response("fetched", Value::String("replay_data".into()));

        // Run: fetch → callback
        let (_, callbacks) = harness
            .send_with_effects(
                AppMessage::new("fetch", Value::String(String::new())),
                &mut executor,
            )
            .unwrap();
        for cb in &callbacks {
            harness.send(cb.clone()).unwrap();
        }

        // Build full message sequence for replay
        let mut all_messages = vec![AppMessage::new("fetch", Value::String(String::new()))];
        all_messages.extend(callbacks);

        // Replay should match
        let identical = harness.replay_verify(EFFECT_APP, all_messages).unwrap();
        assert!(identical, "replay must produce identical states");
    }
}
