#[cfg(test)]
mod tests {
    use boruna_bytecode::*;
    use crate::vm::Vm;
    use crate::capability_gateway::*;
    use crate::error::VmError;
    use crate::replay::*;

    fn simple_module(code: Vec<Op>, constants: Vec<Value>) -> Module {
        let mut module = Module::new("test");
        module.constants = constants;
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 4,
            code,
            capabilities: vec![
                Capability::TimeNow,
                Capability::NetFetch,
                Capability::FsRead,
            ],
            match_tables: vec![],
        });
        module
    }

    fn run_module(module: Module) -> Result<Value, crate::error::VmError> {
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.run()
    }

    #[test]
    fn test_arithmetic() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // 10
                Op::PushConst(1), // 3
                Op::Add,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(3)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(13));
    }

    #[test]
    fn test_subtraction() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Sub,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(3)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(7));
    }

    #[test]
    fn test_multiplication() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Mul,
                Op::Ret,
            ],
            vec![Value::Int(6), Value::Int(7)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_division() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Div,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(3)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(3));
    }

    #[test]
    fn test_division_by_zero() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Div,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(0)],
        );
        assert!(run_module(module).is_err());
    }

    #[test]
    fn test_comparison() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Lt,
                Op::Ret,
            ],
            vec![Value::Int(3), Value::Int(10)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_local_variables() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // 42
                Op::StoreLocal(0),
                Op::LoadLocal(0),
                Op::Ret,
            ],
            vec![Value::Int(42)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_conditional_jump() {
        // 0: PushConst(true)
        // 1: JmpIf(4)    → jump to index 4
        // 2: PushConst("no")
        // 3: Ret
        // 4: PushConst("yes")
        // 5: Ret
        let module = simple_module(
            vec![
                Op::PushConst(0), // true
                Op::JmpIf(4),
                Op::PushConst(1), // "no"
                Op::Ret,
                Op::PushConst(2), // "yes"
                Op::Ret,
            ],
            vec![Value::Bool(true), Value::String("no".into()), Value::String("yes".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("yes".into()));
    }

    #[test]
    fn test_string_concat() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Concat,
                Op::Ret,
            ],
            vec![Value::String("hello ".into()), Value::String("world".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("hello world".into()));
    }

    #[test]
    fn test_record_creation_and_field_access() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // "Alice"
                Op::PushConst(1), // 30
                Op::MakeRecord(0, 2),
                Op::GetField(0),  // get first field
                Op::Ret,
            ],
            vec![Value::String("Alice".into()), Value::Int(30)],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("Alice".into()));
    }

    #[test]
    fn test_function_call() {
        let mut module = Module::new("test");
        module.constants = vec![Value::Int(10), Value::Int(20)];

        // Function 0: main — calls add(10, 20)
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Call(1, 2),
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });

        // Function 1: add(a, b)
        module.add_function(Function {
            name: "add".into(),
            arity: 2,
            locals: 2,
            code: vec![
                Op::LoadLocal(0),
                Op::LoadLocal(1),
                Op::Add,
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });

        assert_eq!(run_module(module).unwrap(), Value::Int(30));
    }

    #[test]
    fn test_negation() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Neg,
                Op::Ret,
            ],
            vec![Value::Int(42)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(-42));
    }

    #[test]
    fn test_logical_not() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Not,
                Op::Ret,
            ],
            vec![Value::Bool(true)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_halt() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Halt,
                Op::PushConst(1), // should never reach
                Op::Ret,
            ],
            vec![Value::Int(42), Value::Int(99)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_stack_underflow() {
        let module = simple_module(
            vec![Op::Pop, Op::Ret],
            vec![],
        );
        assert!(run_module(module).is_err());
    }

    #[test]
    fn test_execution_limit() {
        // Infinite loop
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Pop,
                Op::Jmp(0),
            ],
            vec![Value::Int(0)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_max_steps(100);
        assert!(vm.run().is_err());
    }

    #[test]
    fn test_capability_denied() {
        let module = simple_module(
            vec![
                Op::CapCall(0, 0), // net.fetch — not in deny-all policy
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::deny_all());
        let mut vm = Vm::new(module, gateway);
        assert!(vm.run().is_err());
    }

    #[test]
    fn test_capability_budget() {
        let module = simple_module(
            vec![
                Op::CapCall(5, 0), // time.now
                Op::Pop,
                Op::CapCall(5, 0), // time.now again — exceeds budget
                Op::Ret,
            ],
            vec![],
        );
        let mut policy = Policy::deny_all();
        policy.allow(&Capability::TimeNow, 1); // budget of 1
        let gateway = CapabilityGateway::new(policy);
        let mut vm = Vm::new(module, gateway);
        assert!(vm.run().is_err());
    }

    #[test]
    fn test_emit_ui() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Dup,
                Op::EmitUi,
                Op::Ret,
            ],
            vec![Value::String("hello ui".into())],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        let result = vm.run().unwrap();
        assert_eq!(result, Value::String("hello ui".into()));
        assert_eq!(vm.ui_output.len(), 1);
    }

    #[test]
    fn test_assert_pass() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // true
                Op::Assert(1),
                Op::PushConst(2),
                Op::Ret,
            ],
            vec![Value::Bool(true), Value::String("should not fail".into()), Value::Int(42)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_assert_fail() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // false
                Op::Assert(1),
                Op::PushConst(2),
                Op::Ret,
            ],
            vec![Value::Bool(false), Value::String("oops".into()), Value::Int(42)],
        );
        assert!(run_module(module).is_err());
    }

    #[test]
    fn test_determinism() {
        // Run the same module twice, verify identical step counts
        let make_module = || {
            simple_module(
                vec![
                    Op::PushConst(0),
                    Op::PushConst(1),
                    Op::Add,
                    Op::PushConst(0),
                    Op::Mul,
                    Op::Ret,
                ],
                vec![Value::Int(7), Value::Int(6)],
            )
        };

        let gateway1 = CapabilityGateway::new(Policy::allow_all());
        let mut vm1 = Vm::new(make_module(), gateway1);
        let r1 = vm1.run().unwrap();

        let gateway2 = CapabilityGateway::new(Policy::allow_all());
        let mut vm2 = Vm::new(make_module(), gateway2);
        let r2 = vm2.run().unwrap();

        assert_eq!(r1, r2);
        assert_eq!(vm1.step_count(), vm2.step_count());
    }

    #[test]
    fn test_event_log_json_roundtrip() {
        let mut log = EventLog::new();
        log.log_cap_call(&Capability::TimeNow, &[]);
        log.log_cap_result(&Capability::TimeNow, &Value::Int(12345));

        let json = log.to_json().unwrap();
        let restored = EventLog::from_json(&json).unwrap();
        assert_eq!(log.events().len(), restored.events().len());
    }

    #[test]
    fn test_replay_verification() {
        let mut log1 = EventLog::new();
        log1.log_cap_call(&Capability::TimeNow, &[]);
        log1.log_cap_result(&Capability::TimeNow, &Value::Int(100));

        let mut log2 = EventLog::new();
        log2.log_cap_call(&Capability::TimeNow, &[]);
        log2.log_cap_result(&Capability::TimeNow, &Value::Int(100));

        let result = ReplayEngine::verify(&log1, &log2);
        assert!(matches!(result, ReplayResult::Identical));
    }

    #[test]
    fn test_replay_divergence() {
        let mut log1 = EventLog::new();
        log1.log_cap_call(&Capability::TimeNow, &[]);

        let mut log2 = EventLog::new();
        log2.log_cap_call(&Capability::NetFetch, &[Value::String("url".into())]);

        let result = ReplayEngine::verify(&log1, &log2);
        assert!(matches!(result, ReplayResult::Diverged { .. }));
    }

    #[test]
    fn test_trace_enabled() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Ret,
            ],
            vec![Value::Int(1)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.trace_enabled = true;
        vm.run().unwrap();
        assert!(!vm.trace.is_empty());
    }

    // ── List operation tests ──

    #[test]
    fn test_make_list_empty() {
        let module = simple_module(
            vec![
                Op::MakeList(0),
                Op::Ret,
            ],
            vec![],
        );
        assert_eq!(run_module(module).unwrap(), Value::List(vec![]));
    }

    #[test]
    fn test_make_list_with_elements() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // 10
                Op::PushConst(1), // 20
                Op::PushConst(2), // 30
                Op::MakeList(3),
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)])
        );
    }

    #[test]
    fn test_list_len() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::MakeList(2),
                Op::ListLen,
                Op::Ret,
            ],
            vec![Value::Int(1), Value::Int(2)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(2));
    }

    #[test]
    fn test_list_len_empty() {
        let module = simple_module(
            vec![
                Op::MakeList(0),
                Op::ListLen,
                Op::Ret,
            ],
            vec![],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_list_get() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // "a"
                Op::PushConst(1), // "b"
                Op::PushConst(2), // "c"
                Op::MakeList(3),
                Op::PushConst(3), // index 1
                Op::ListGet,
                Op::Ret,
            ],
            vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
                Value::Int(1),
            ],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("b".into()));
    }

    #[test]
    fn test_list_get_out_of_bounds() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::MakeList(1),
                Op::PushConst(1), // index 5
                Op::ListGet,
                Op::Ret,
            ],
            vec![Value::Int(42), Value::Int(5)],
        );
        let result = run_module(module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("index out of bounds"));
    }

    #[test]
    fn test_list_get_negative_index() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::MakeList(1),
                Op::PushConst(1), // index -1
                Op::ListGet,
                Op::Ret,
            ],
            vec![Value::Int(42), Value::Int(-1)],
        );
        let result = run_module(module);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_push() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // 1
                Op::MakeList(1),
                Op::PushConst(1), // 2
                Op::ListPush,
                Op::Ret,
            ],
            vec![Value::Int(1), Value::Int(2)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }

    #[test]
    fn test_list_push_to_empty() {
        let module = simple_module(
            vec![
                Op::MakeList(0),
                Op::PushConst(0), // 42
                Op::ListPush,
                Op::Ret,
            ],
            vec![Value::Int(42)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(42)])
        );
    }

    #[test]
    fn test_list_len_on_legacy_record() {
        // Legacy MakeRecord(0xFFFF) should still work with ListLen
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::MakeRecord(0xFFFF, 2),
                Op::ListLen,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(20)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(2));
    }

    // ── String builtin tests ──

    #[test]
    fn test_parse_int_valid() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::ParseInt,
                Op::Ret,
            ],
            vec![Value::String("42".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_parse_int_negative() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::ParseInt,
                Op::Ret,
            ],
            vec![Value::String("-7".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(-7));
    }

    #[test]
    fn test_parse_int_invalid() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::ParseInt,
                Op::Ret,
            ],
            vec![Value::String("hello".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_parse_int_empty() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::ParseInt,
                Op::Ret,
            ],
            vec![Value::String("".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_parse_int_with_whitespace() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::ParseInt,
                Op::Ret,
            ],
            vec![Value::String("  123  ".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(123));
    }

    #[test]
    fn test_try_parse_int_valid() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::TryParseInt,
                Op::Ret,
            ],
            vec![Value::String("42".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Ok(Box::new(Value::Int(42))));
    }

    #[test]
    fn test_try_parse_int_invalid() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::TryParseInt,
                Op::Ret,
            ],
            vec![Value::String("hello".into())],
        );
        match run_module(module).unwrap() {
            Value::Err(_) => {} // expected
            other => panic!("expected Err, got {other}"),
        }
    }

    #[test]
    fn test_try_parse_int_empty() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::TryParseInt,
                Op::Ret,
            ],
            vec![Value::String("".into())],
        );
        match run_module(module).unwrap() {
            Value::Err(_) => {} // expected
            other => panic!("expected Err, got {other}"),
        }
    }

    #[test]
    fn test_str_contains_true() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // "hello world"
                Op::PushConst(1), // "world"
                Op::StrContains,
                Op::Ret,
            ],
            vec![Value::String("hello world".into()), Value::String("world".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_str_contains_false() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::StrContains,
                Op::Ret,
            ],
            vec![Value::String("hello".into()), Value::String("xyz".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_str_contains_empty_needle() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::StrContains,
                Op::Ret,
            ],
            vec![Value::String("hello".into()), Value::String("".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_str_starts_with_true() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::StrStartsWith,
                Op::Ret,
            ],
            vec![Value::String("conflict:5".into()), Value::String("conflict".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_str_starts_with_false() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::StrStartsWith,
                Op::Ret,
            ],
            vec![Value::String("ok".into()), Value::String("conflict".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    // ── EventLog version and format stability tests ──

    use crate::replay::EVENT_LOG_VERSION;

    #[test]
    fn test_event_log_includes_version() {
        let log = EventLog::new();
        assert_eq!(log.version(), EVENT_LOG_VERSION);

        let json = log.to_json().unwrap();
        assert!(json.contains("\"version\""), "JSON must include version field");
    }

    #[test]
    fn test_event_log_version_roundtrip() {
        let mut log = EventLog::new();
        log.log_cap_call(&Capability::TimeNow, &[]);
        log.log_cap_result(&Capability::TimeNow, &Value::Int(100));

        let json = log.to_json().unwrap();
        let restored = EventLog::from_json(&json).unwrap();
        assert_eq!(restored.version(), EVENT_LOG_VERSION);
        assert_eq!(restored.events().len(), 2);
    }

    #[test]
    fn test_event_log_missing_version_defaults_v1() {
        // Simulate old JSON without version field
        let old_json = r#"{"events":[{"CapCall":{"capability":"time.now","args":[]}}]}"#;
        let log = EventLog::from_json(old_json).unwrap();
        assert_eq!(log.version(), EVENT_LOG_VERSION);
        assert_eq!(log.events().len(), 1);
    }

    #[test]
    fn test_event_log_rejects_unknown_version() {
        let future_json = r#"{"version":99,"events":[]}"#;
        let result = EventLog::from_json(future_json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unsupported"), "error should mention unsupported: {err}");
    }

    #[test]
    fn test_event_log_v1_format_stability() {
        // Golden test: lock the JSON format of EventLog v1
        let mut log = EventLog::new();
        log.log_cap_call(&Capability::NetFetch, &[Value::String("https://example.com".into())]);
        log.log_cap_result(&Capability::NetFetch, &Value::String("response".into()));

        let json = log.to_json().unwrap();

        // Must contain version
        assert!(json.contains("\"version\": 1"), "must have version: 1");
        // Must contain events array
        assert!(json.contains("\"events\""), "must have events array");
        // Must contain CapCall variant
        assert!(json.contains("\"CapCall\""), "must have CapCall variant");
        // Must contain capability name
        assert!(json.contains("\"net.fetch\""), "must have capability name");
        // Must contain CapResult variant
        assert!(json.contains("\"CapResult\""), "must have CapResult variant");

        // Roundtrip must preserve exactly
        let restored = EventLog::from_json(&json).unwrap();
        let json2 = restored.to_json().unwrap();
        assert_eq!(json, json2, "JSON must be identical after roundtrip");
    }

    #[test]
    fn test_event_log_format_all_event_types() {
        // Verify all event types serialize/deserialize correctly
        let mut log = EventLog::new();
        log.log_cap_call(&Capability::TimeNow, &[]);
        log.log_cap_result(&Capability::TimeNow, &Value::Int(100));
        log.log_actor_spawn(1, "worker");
        log.log_message_send(0, 1, &Value::String("hello".into()));
        log.log_ui_emit(&Value::String("tree".into()));

        let json = log.to_json().unwrap();
        let restored = EventLog::from_json(&json).unwrap();
        assert_eq!(restored.events().len(), 5);

        // Roundtrip preserves format
        let json2 = restored.to_json().unwrap();
        assert_eq!(json, json2);
    }

    // ========== Phase 1+2: Bounded execution + opcode wiring ==========

    #[test]
    fn test_execute_bounded_completes() {
        // Simple program finishes within budget
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::Ret,
            ],
            vec![Value::Int(42)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(1000) {
            crate::vm::StepResult::Completed(val) => assert_eq!(val, Value::Int(42)),
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_execute_bounded_yields() {
        // Tight loop exceeds budget — returns Yielded
        // Create a loop: jump back to start
        let module = simple_module(
            vec![
                Op::PushConst(0),  // push 1
                Op::Pop,           // pop it
                Op::Jmp(0),        // loop back to start
            ],
            vec![Value::Int(1)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(10) {
            crate::vm::StepResult::Yielded { steps_used } => {
                assert!(steps_used >= 10, "should have used at least 10 steps");
            }
            other => panic!("expected Yielded, got {:?}", other),
        }
    }

    #[test]
    fn test_execute_bounded_backward_compat() {
        // run() still works identically
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::Add,
                Op::Ret,
            ],
            vec![Value::Int(10), Value::Int(32)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_receive_msg_blocks_when_empty() {
        // ReceiveMsg with empty mailbox in bounded mode returns Blocked
        let module = simple_module(
            vec![
                Op::ReceiveMsg,
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Blocked => {} // expected
            other => panic!("expected Blocked, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_msg_pops_from_mailbox() {
        // ReceiveMsg returns message from mailbox
        use crate::actor::Message;
        let module = simple_module(
            vec![
                Op::ReceiveMsg,
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.deliver_message(Message { from: 99, payload: Value::Int(777) });
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => assert_eq!(val, Value::Int(777)),
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_msg_legacy_returns_unit() {
        // In legacy unbounded mode (run()), ReceiveMsg pushes Unit
        let module = simple_module(
            vec![
                Op::ReceiveMsg,
                Op::Ret,
            ],
            vec![],
        );
        assert_eq!(run_module(module).unwrap(), Value::Unit);
    }

    #[test]
    fn test_spawn_actor_returns_unique_ids() {
        // Two spawns return different ActorIds
        // Need a module with at least one extra function to spawn
        let mut module = Module::new("test");
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::PushConst(0), Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(0));
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 2,
            code: vec![
                Op::SpawnActor(0),   // spawn worker -> ActorId(0)
                Op::StoreLocal(0),
                Op::SpawnActor(0),   // spawn worker -> ActorId(1)
                Op::StoreLocal(1),
                Op::LoadLocal(0),    // push first
                Op::LoadLocal(1),    // push second
                // Build record with both IDs
                Op::MakeRecord(0, 2),
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1; // main is at index 1
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        let result = vm.run().unwrap();
        match result {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::ActorId(0));
                assert_eq!(fields[1], Value::ActorId(1));
            }
            _ => panic!("expected Record"),
        }
    }

    #[test]
    fn test_spawn_actor_creates_request() {
        // spawn_requests Vec is populated
        let mut module = Module::new("test");
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::PushConst(0), Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(0));
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0),  // spawn worker
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1; // main is at index 1
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        let _ = vm.run().unwrap();
        let requests = vm.drain_spawn_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].func_idx, 0);
    }

    #[test]
    fn test_send_msg_queues_outgoing() {
        // SendMsg populates outgoing_messages
        let module = simple_module(
            vec![
                Op::PushConst(0),  // target: ActorId(5)
                Op::PushConst(1),  // payload: Int(42)
                Op::SendMsg,
                Op::PushConst(2),  // return 0
                Op::Ret,
            ],
            vec![Value::ActorId(5), Value::Int(42), Value::Int(0)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        let _ = vm.run().unwrap();
        let messages = vm.drain_outgoing_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, 5); // target
        assert_eq!(messages[0].1, Value::Int(42)); // payload
    }

    #[test]
    fn test_send_msg_type_error() {
        // Sending to non-ActorId produces TypeError
        let module = simple_module(
            vec![
                Op::PushConst(0),  // target: Int(5) -- not ActorId!
                Op::PushConst(1),  // payload
                Op::SendMsg,
                Op::PushConst(0),
                Op::Ret,
            ],
            vec![Value::Int(5), Value::Int(42)],
        );
        let result = run_module(module);
        assert!(result.is_err());
        match result.unwrap_err() {
            VmError::TypeError { expected, .. } => assert_eq!(expected, "ActorId"),
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_msg_consumes_fifo() {
        // Messages consumed in FIFO order
        use crate::actor::Message;
        let module = simple_module(
            vec![
                Op::ReceiveMsg,     // first message
                Op::StoreLocal(0),
                Op::ReceiveMsg,     // second message
                Op::StoreLocal(1),
                Op::LoadLocal(0),
                Op::LoadLocal(1),
                Op::MakeRecord(0, 2),
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.deliver_message(Message { from: 0, payload: Value::Int(111) });
        vm.deliver_message(Message { from: 0, payload: Value::Int(222) });
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => {
                match val {
                    Value::Record { fields, .. } => {
                        assert_eq!(fields[0], Value::Int(111));
                        assert_eq!(fields[1], Value::Int(222));
                    }
                    _ => panic!("expected Record"),
                }
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_blocks_then_resumes() {
        // Actor blocks on receive, then resumes after message delivery
        use crate::actor::Message;
        let module = simple_module(
            vec![
                Op::ReceiveMsg,  // will block first time
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_entry_function(0).unwrap();

        // First attempt: blocks
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Blocked => {} // expected
            other => panic!("expected Blocked, got {:?}", other),
        }

        // Deliver a message
        vm.deliver_message(Message { from: 0, payload: Value::Int(999) });

        // Second attempt: completes
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => assert_eq!(val, Value::Int(999)),
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    // ========== Phase 3: ActorSystem scheduler tests ==========

    #[test]
    fn test_single_actor_backward_compat() {
        // ActorSystem::run() with one actor should produce same result as run_single()
        let module = simple_module(
            vec![
                Op::PushConst(0), // 42
                Op::Ret,
            ],
            vec![Value::Int(42)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_two_actors_round_robin() {
        // Parent spawns child, both complete
        let mut module = Module::new("test");
        // Function 0: worker — returns 99
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::PushConst(0), Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(99));
        // Function 1: main — spawns worker, returns 42
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0), // spawn worker
                Op::Pop,           // discard ActorId
                Op::PushConst(1),  // 42
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(42));
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        assert_eq!(result, Value::Int(42));
        assert_eq!(system.actor_count(), 2);
    }

    #[test]
    fn test_message_passing_parent_to_child() {
        // Parent spawns child, sends message, child receives and returns it
        let mut module = Module::new("test");
        // Function 0: worker — receives a message and returns payload
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::ReceiveMsg,  // blocks until message arrives
                Op::Ret,         // return payload
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        // Function 1: main — spawns worker, sends Int(77), returns 0
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 1,
            code: vec![
                Op::SpawnActor(0),  // spawn worker -> ActorId
                Op::StoreLocal(0),  // save child id
                Op::LoadLocal(0),   // push target
                Op::PushConst(0),   // payload: Int(77)
                Op::SendMsg,        // send to child
                Op::PushConst(1),   // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(77));
        module.constants.push(Value::Int(0));
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        assert_eq!(result, Value::Int(0));
    }

    #[test]
    fn test_message_passing_child_to_parent() {
        // Child sends message to parent (actor id 0), parent receives it
        let mut module = Module::new("test");
        // Function 0: worker — sends Int(55) to actor 0 (parent), returns 0
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0),   // target: ActorId(0) = parent
                Op::PushConst(1),   // payload: Int(55)
                Op::SendMsg,
                Op::PushConst(2),   // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::ActorId(0));
        module.constants.push(Value::Int(55));
        module.constants.push(Value::Int(0));
        // Function 1: main — spawns worker, receives message from child
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0),  // spawn worker
                Op::Pop,            // discard child ActorId
                Op::ReceiveMsg,     // blocks until child's message arrives
                Op::Ret,            // return received payload
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        assert_eq!(result, Value::Int(55));
    }

    #[test]
    fn test_deadlock_detection() {
        // Two actors both blocked on ReceiveMsg with no messages pending → deadlock
        let mut module = Module::new("test");
        // Function 0: worker — just blocks on receive
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::ReceiveMsg, Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });
        // Function 1: main — spawns worker, then blocks on receive
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0),
                Op::Pop,
                Op::ReceiveMsg,
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run();
        match result {
            Err(VmError::Deadlock) => {} // expected
            other => panic!("expected Deadlock, got {:?}", other),
        }
    }

    #[test]
    fn test_max_rounds_exceeded() {
        // Ping-pong: parent and child send messages back and forth forever
        let mut module = Module::new("test");
        // Function 0: worker — receive, send back to parent, loop
        // We use a simple loop: receive -> send to 0 -> jump back
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::ReceiveMsg,     // 0: receive
                Op::PushConst(0),   // 1: target = ActorId(0) (parent)
                Op::PushConst(1),   // 2: payload = Int(1)
                Op::SendMsg,        // 3: send
                Op::Jmp(0),        // 4: loop back
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::ActorId(0));
        module.constants.push(Value::Int(1));
        // Function 1: main — spawns worker, sends initial message, loops receiving
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 1,
            code: vec![
                Op::SpawnActor(0),  // 0: spawn worker
                Op::StoreLocal(0),  // 1: save child id
                Op::LoadLocal(0),   // 2: push child id
                Op::PushConst(1),   // 3: payload = Int(1)
                Op::SendMsg,        // 4: send initial message
                Op::ReceiveMsg,     // 5: receive reply
                Op::LoadLocal(0),   // 6: push child id
                Op::PushConst(1),   // 7: payload
                Op::SendMsg,        // 8: send again
                Op::Jmp(5),        // 9: loop back to receive
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.set_max_rounds(50);
        system.spawn_root(module, gateway);
        let result = system.run();
        match result {
            Err(VmError::MaxRoundsExceeded(50)) => {} // expected
            other => panic!("expected MaxRoundsExceeded(50), got {:?}", other),
        }
    }

    #[test]
    fn test_blocked_actor_wakes_on_message() {
        // Actor blocks on ReceiveMsg, then wakes when a message is delivered
        // This is implicitly tested by test_message_passing_child_to_parent
        // but let's test explicitly with external message injection
        let module = simple_module(
            vec![
                Op::ReceiveMsg, // blocks
                Op::Ret,        // return received value
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        // Inject a message from external (actor 99)
        system.send(0, crate::actor::Message {
            from: 99,
            payload: Value::Int(123),
        }).unwrap();
        let result = system.run().unwrap();
        assert_eq!(result, Value::Int(123));
    }

    #[test]
    fn test_message_delivery_order_deterministic() {
        // Multiple senders to the same target — delivery order is deterministic
        // (sorted by target_id, sender_id)
        let mut module = Module::new("test");
        // Function 0: worker_a — sends Int(1) to parent
        module.add_function(Function {
            name: "worker_a".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0),  // ActorId(0)
                Op::PushConst(1),  // Int(1)
                Op::SendMsg,
                Op::PushConst(3),  // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        // Function 1: worker_b — sends Int(2) to parent
        module.add_function(Function {
            name: "worker_b".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0),  // ActorId(0)
                Op::PushConst(2),  // Int(2)
                Op::SendMsg,
                Op::PushConst(3),  // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::ActorId(0)); // 0: parent id
        module.constants.push(Value::Int(1));       // 1
        module.constants.push(Value::Int(2));       // 2
        module.constants.push(Value::Int(0));       // 3
        // Function 2: main — spawns both workers, receives two messages
        // Returns a list [first_msg, second_msg]
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 2,
            code: vec![
                Op::SpawnActor(0),   // spawn worker_a
                Op::Pop,
                Op::SpawnActor(1),   // spawn worker_b
                Op::Pop,
                Op::ReceiveMsg,      // first message
                Op::StoreLocal(0),
                Op::ReceiveMsg,      // second message
                Op::StoreLocal(1),
                Op::LoadLocal(0),
                Op::LoadLocal(1),
                Op::MakeRecord(0, 2), // pack into record
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 2;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        // Messages sorted by (target=0, sender_id). worker_a has lower id than worker_b.
        // So first message is from worker_a (Int(1)), second from worker_b (Int(2)).
        match result {
            Value::Record { fields, .. } => {
                assert_eq!(fields[0], Value::Int(1)); // from worker_a (lower id)
                assert_eq!(fields[1], Value::Int(2)); // from worker_b (higher id)
            }
            other => panic!("expected Record, got {:?}", other),
        }
    }

    // ========== Phase 4: EventLog + Replay tests ==========

    #[test]
    fn test_log_message_receive_roundtrip() {
        let mut log = EventLog::new();
        log.log_message_receive(42, &Value::String("hello".into()));
        let json = log.to_json().unwrap();
        let log2 = EventLog::from_json(&json).unwrap();
        assert_eq!(log2.events().len(), 1);
        match &log2.events()[0] {
            Event::MessageReceive { actor_id, payload } => {
                assert_eq!(*actor_id, 42);
                assert_eq!(*payload, Value::String("hello".into()));
            }
            other => panic!("expected MessageReceive, got {:?}", other),
        }
    }

    #[test]
    fn test_log_scheduler_tick_roundtrip() {
        let mut log = EventLog::new();
        log.log_scheduler_tick(5, 3);
        let json = log.to_json().unwrap();
        let log2 = EventLog::from_json(&json).unwrap();
        assert_eq!(log2.events().len(), 1);
        match &log2.events()[0] {
            Event::SchedulerTick { round, active_actor } => {
                assert_eq!(*round, 5);
                assert_eq!(*active_actor, 3);
            }
            other => panic!("expected SchedulerTick, got {:?}", other),
        }
    }

    #[test]
    fn test_replay_verify_full_identical() {
        let mut log = EventLog::new();
        log.log_scheduler_tick(0, 0);
        log.log_actor_spawn(1, "worker");
        log.log_message_send(0, 1, &Value::Int(42));
        log.log_message_receive(1, &Value::Int(42));

        // Clone via JSON roundtrip
        let json = log.to_json().unwrap();
        let log2 = EventLog::from_json(&json).unwrap();

        match ReplayEngine::verify_full(&log, &log2) {
            ReplayResult::Identical => {} // expected
            ReplayResult::Diverged { reason } => panic!("expected Identical, got Diverged: {}", reason),
        }
    }

    #[test]
    fn test_replay_verify_full_diverged_scheduler() {
        let mut log1 = EventLog::new();
        log1.log_scheduler_tick(0, 0);
        log1.log_scheduler_tick(0, 1);

        let mut log2 = EventLog::new();
        log2.log_scheduler_tick(0, 1); // different order
        log2.log_scheduler_tick(0, 0);

        match ReplayEngine::verify_full(&log1, &log2) {
            ReplayResult::Diverged { .. } => {} // expected
            ReplayResult::Identical => panic!("expected Diverged, got Identical"),
        }
    }

    #[test]
    fn test_replay_verify_full_diverged_messages() {
        let mut log1 = EventLog::new();
        log1.log_message_send(0, 1, &Value::Int(10));
        log1.log_message_send(0, 2, &Value::Int(20));

        let mut log2 = EventLog::new();
        log2.log_message_send(0, 2, &Value::Int(20)); // different order
        log2.log_message_send(0, 1, &Value::Int(10));

        match ReplayEngine::verify_full(&log1, &log2) {
            ReplayResult::Diverged { .. } => {} // expected
            ReplayResult::Identical => panic!("expected Diverged, got Identical"),
        }
    }

    #[test]
    fn test_scheduler_event_log() {
        // Run a parent-child scenario and verify events are logged
        let mut module = Module::new("test");
        // Function 0: worker — sends Int(99) to parent, returns 0
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0),  // ActorId(0)
                Op::PushConst(1),  // Int(99)
                Op::SendMsg,
                Op::PushConst(2),  // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::ActorId(0));
        module.constants.push(Value::Int(99));
        module.constants.push(Value::Int(0));
        // Function 1: main — spawns worker, receives message, returns it
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0),
                Op::Pop,
                Op::ReceiveMsg,
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let _result = system.run().unwrap();

        let events = system.event_log().events();
        // Should have: SchedulerTick(s), ActorSpawn, more SchedulerTick(s), MessageSend, MessageReceive
        let has_scheduler_tick = events.iter().any(|e| matches!(e, Event::SchedulerTick { .. }));
        let has_actor_spawn = events.iter().any(|e| matches!(e, Event::ActorSpawn { .. }));
        let has_message_send = events.iter().any(|e| matches!(e, Event::MessageSend { .. }));
        let has_message_receive = events.iter().any(|e| matches!(e, Event::MessageReceive { .. }));
        assert!(has_scheduler_tick, "expected SchedulerTick events");
        assert!(has_actor_spawn, "expected ActorSpawn events");
        assert!(has_message_send, "expected MessageSend events");
        assert!(has_message_receive, "expected MessageReceive events");
    }

    // ========== Phase 7: Supervision tests ==========

    #[test]
    fn test_supervision_child_crash_notifies_parent() {
        // Child divides by zero → parent receives error message
        let mut module = Module::new("test");
        // Function 0: worker — divides by zero (crashes)
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::PushConst(0), // 1
                Op::PushConst(1), // 0
                Op::Div,          // division by zero!
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(1));
        module.constants.push(Value::Int(0));
        // Function 1: main — spawns worker, receives error notification
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0),
                Op::Pop,
                Op::ReceiveMsg,  // receives error from crashed child
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 1;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        // Parent receives Err(String) from the crashed child
        match result {
            Value::Err(inner) => {
                match *inner {
                    Value::String(s) => assert!(s.contains("division by zero"), "expected division by zero error, got: {s}"),
                    other => panic!("expected Err(String), got Err({:?})", other),
                }
            }
            other => panic!("expected Err value, got {:?}", other),
        }
    }

    #[test]
    fn test_supervision_cascade_failure() {
        // Child spawns grandchild, then child crashes.
        // Both child and grandchild should be failed (cascade).
        // Parent blocked on receive → deadlock (no one left to send).
        let mut module = Module::new("test");
        // Function 0: grandchild — just waits for a message (will be cascade-failed)
        module.add_function(Function {
            name: "grandchild".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::ReceiveMsg, Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });
        // Function 1: child — spawns grandchild, then crashes (div by zero)
        module.add_function(Function {
            name: "child".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(0), // spawn grandchild
                Op::Pop,
                Op::PushConst(0),  // 1
                Op::PushConst(1),  // 0
                Op::Div,           // crash!
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::Int(1));
        module.constants.push(Value::Int(0));
        // Function 2: main — spawns child, receives error notification
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::SpawnActor(1), // spawn child
                Op::Pop,
                Op::ReceiveMsg,    // receives error from crashed child
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.entry = 2;

        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut system = crate::actor::ActorSystem::new();
        system.spawn_root(module, gateway);
        let result = system.run().unwrap();
        // Parent receives error notification from crashed child
        match result {
            Value::Err(inner) => {
                match *inner {
                    Value::String(s) => assert!(s.contains("division by zero"), "expected division by zero, got: {s}"),
                    other => panic!("expected Err(String), got Err({:?})", other),
                }
            }
            other => panic!("expected Err from cascade, got {:?}", other),
        }
        // 2 actors total: root(0), child(1). Grandchild was never spawned
        // because child crashed — spawn requests from failed actors are discarded.
        assert_eq!(system.actor_count(), 2);
    }

    #[test]
    fn test_deadlock_error_display() {
        let err = VmError::Deadlock;
        assert_eq!(format!("{err}"), "deadlock: all actors blocked with no pending messages");
    }

    #[test]
    fn test_max_rounds_error_display() {
        let err = VmError::MaxRoundsExceeded(100);
        assert_eq!(format!("{err}"), "max scheduler rounds exceeded (100)");
    }
}
