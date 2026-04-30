#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::capability_gateway::*;
    use crate::error::VmError;
    use crate::replay::*;
    use crate::vm::Vm;
    use boruna_bytecode::*;

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
            vec![Op::PushConst(0), Op::PushConst(1), Op::Sub, Op::Ret],
            vec![Value::Int(10), Value::Int(3)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(7));
    }

    #[test]
    fn test_multiplication() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::Mul, Op::Ret],
            vec![Value::Int(6), Value::Int(7)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_division() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::Div, Op::Ret],
            vec![Value::Int(10), Value::Int(3)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(3));
    }

    #[test]
    fn test_division_by_zero() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::Div, Op::Ret],
            vec![Value::Int(10), Value::Int(0)],
        );
        assert!(run_module(module).is_err());
    }

    #[test]
    fn test_comparison() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::Lt, Op::Ret],
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
            vec![
                Value::Bool(true),
                Value::String("no".into()),
                Value::String("yes".into()),
            ],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("yes".into()));
    }

    #[test]
    fn test_string_concat() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::Concat, Op::Ret],
            vec![
                Value::String("hello ".into()),
                Value::String("world".into()),
            ],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("hello world".into())
        );
    }

    #[test]
    fn test_record_creation_and_field_access() {
        let module = simple_module(
            vec![
                Op::PushConst(0), // "Alice"
                Op::PushConst(1), // 30
                Op::MakeRecord(0, 2),
                Op::GetField(0), // get first field
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
            code: vec![Op::PushConst(0), Op::PushConst(1), Op::Call(1, 2), Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });

        // Function 1: add(a, b)
        module.add_function(Function {
            name: "add".into(),
            arity: 2,
            locals: 2,
            code: vec![Op::LoadLocal(0), Op::LoadLocal(1), Op::Add, Op::Ret],
            capabilities: vec![],
            match_tables: vec![],
        });

        assert_eq!(run_module(module).unwrap(), Value::Int(30));
    }

    #[test]
    fn test_negation() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::Neg, Op::Ret],
            vec![Value::Int(42)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(-42));
    }

    #[test]
    fn test_logical_not() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::Not, Op::Ret],
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
        let module = simple_module(vec![Op::Pop, Op::Ret], vec![]);
        assert!(run_module(module).is_err());
    }

    #[test]
    fn test_execution_limit() {
        // Infinite loop
        let module = simple_module(
            vec![Op::PushConst(0), Op::Pop, Op::Jmp(0)],
            vec![Value::Int(0)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_max_steps(100);
        assert!(vm.run().is_err());
    }

    // ── 0.4-S5: capability span emission ──

    /// Test helper: a span-capture layer keyed by span Id (proper matching,
    /// not the best-effort matcher the first draft used). Records both
    /// `on_new_span` (initial attributes) and `on_record` (later
    /// `Span::record` calls) into per-span buckets.
    #[derive(Default, Clone)]
    struct CapturedSpan {
        name: String,
        cap_name: Option<String>,
        bytes_in: Option<u64>,
        bytes_out: Option<u64>,
        budget_remaining: Option<u64>,
        error_kind: Option<String>,
    }

    fn run_with_capture<F: FnOnce()>(work: F) -> Vec<CapturedSpan> {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};
        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record};
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::Layer;

        struct V<'a>(&'a mut CapturedSpan);
        impl Visit for V<'_> {
            fn record_str(&mut self, field: &Field, value: &str) {
                match field.name() {
                    "cap.name" => self.0.cap_name = Some(value.to_string()),
                    "error.kind" => self.0.error_kind = Some(value.to_string()),
                    _ => {}
                }
            }
            fn record_u64(&mut self, field: &Field, value: u64) {
                match field.name() {
                    "bytes_in" => self.0.bytes_in = Some(value),
                    "bytes_out" => self.0.bytes_out = Some(value),
                    "cap.budget_remaining" => self.0.budget_remaining = Some(value),
                    _ => {}
                }
            }
            fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        }

        struct CaptureLayer {
            by_id: Arc<Mutex<HashMap<u64, CapturedSpan>>>,
            order: Arc<Mutex<Vec<u64>>>,
        }
        impl<S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S>
            for CaptureLayer
        {
            fn on_new_span(
                &self,
                attrs: &Attributes<'_>,
                id: &Id,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut span = CapturedSpan {
                    name: attrs.metadata().name().to_string(),
                    ..Default::default()
                };
                attrs.record(&mut V(&mut span));
                let key = id.into_u64();
                self.by_id.lock().unwrap().insert(key, span);
                self.order.lock().unwrap().push(key);
            }
            fn on_record(
                &self,
                id: &Id,
                values: &Record<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let key = id.into_u64();
                let mut by_id = self.by_id.lock().unwrap();
                if let Some(span) = by_id.get_mut(&key) {
                    values.record(&mut V(span));
                }
            }
        }

        let by_id: Arc<Mutex<HashMap<u64, CapturedSpan>>> = Arc::new(Mutex::new(HashMap::new()));
        let order: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let layer = CaptureLayer {
            by_id: by_id.clone(),
            order: order.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, work);

        let spans = by_id.lock().unwrap();
        let order = order.lock().unwrap();
        order
            .iter()
            .filter_map(|id| spans.get(id).cloned())
            .collect()
    }

    #[test]
    fn test_capability_call_emits_boruna_cap_span_with_attributes() {
        let spans = run_with_capture(|| {
            let mut gateway = CapabilityGateway::new(Policy::allow_all());
            let mut log = EventLog::new();
            gateway
                .call(&Capability::TimeNow, &[], &mut log)
                .expect("must succeed under allow-all");
        });

        let cap = spans
            .iter()
            .find(|s| s.name == "boruna.cap")
            .expect("expected boruna.cap span");
        assert_eq!(cap.cap_name.as_deref(), Some("time.now"));
        assert_eq!(cap.bytes_in, Some(0), "TimeNow has no args");
        assert_eq!(
            cap.bytes_out,
            Some(0),
            "TimeNow returns Int (no string payload) → bytes_out=0 (not None — the field MUST have been recorded)"
        );
        assert_eq!(
            cap.error_kind, None,
            "successful call leaves error.kind unrecorded"
        );
    }

    #[test]
    fn test_capability_call_records_bytes_out_for_string_returning_handler() {
        // Calls a capability whose mock handler returns a Value::String — so
        // bytes_out should be the string's UTF-8 length. NetFetch's mock
        // returns a JSON-ish string with the URL embedded. Pick FsRead
        // because its mock returns `"mock file content for {path}"` — a
        // predictable shape we can size.
        let spans = run_with_capture(|| {
            let mut gateway = CapabilityGateway::new(Policy::allow_all());
            let mut log = EventLog::new();
            gateway
                .call(
                    &Capability::FsRead,
                    &[Value::String("/tmp/x".into())],
                    &mut log,
                )
                .expect("must succeed");
        });
        let cap = spans
            .iter()
            .find(|s| s.name == "boruna.cap")
            .expect("expected span");
        // Args: ["/tmp/x"] → 6 bytes
        assert_eq!(cap.bytes_in, Some(6));
        // Output: "mock file content for /tmp/x" → 28 bytes
        let out = cap.bytes_out.expect("bytes_out must be recorded");
        assert!(
            out > 0,
            "string-returning handler must produce non-zero bytes_out, got {out}"
        );
    }

    #[test]
    fn test_capability_call_records_error_kind_denied() {
        let spans = run_with_capture(|| {
            let mut gateway = CapabilityGateway::new(Policy::deny_all());
            let mut log = EventLog::new();
            let _ = gateway.call(&Capability::NetFetch, &[], &mut log);
        });
        let cap = spans
            .iter()
            .find(|s| s.name == "boruna.cap")
            .expect("expected span");
        assert_eq!(
            cap.error_kind.as_deref(),
            Some("denied"),
            "deny-all policy must record error.kind=denied"
        );
    }

    #[test]
    fn test_capability_call_records_error_kind_budget_exceeded() {
        let spans = run_with_capture(|| {
            let mut policy = Policy::allow_all();
            // Budget of 1 — first call OK, second rejected.
            policy.allow(&Capability::TimeNow, 1);
            let mut gateway = CapabilityGateway::new(policy);
            let mut log = EventLog::new();
            // First call: succeeds, span has cap.budget_remaining=0
            gateway.call(&Capability::TimeNow, &[], &mut log).unwrap();
            // Second call: rejected, span has error.kind=budget_exceeded
            let _ = gateway.call(&Capability::TimeNow, &[], &mut log);
        });
        // Two boruna.cap spans — the second one is the rejection.
        let caps: Vec<_> = spans.iter().filter(|s| s.name == "boruna.cap").collect();
        assert_eq!(caps.len(), 2, "expected 2 boruna.cap spans");
        assert_eq!(
            caps[0].budget_remaining,
            Some(0),
            "first call: budget exhausted post-call"
        );
        assert_eq!(caps[0].error_kind, None, "first call succeeded");
        assert_eq!(
            caps[1].error_kind.as_deref(),
            Some("budget_exceeded"),
            "second call must record budget_exceeded"
        );
        assert_eq!(
            caps[1].budget_remaining,
            Some(0),
            "rejected call also reports 0 remaining"
        );
    }

    #[test]
    fn test_capability_call_works_without_subscriber_installed() {
        // The "zero-cost when no subscriber" path: making capability calls
        // with no tracing subscriber installed must not panic, must not
        // allocate gratuitously, and must return the expected result. This
        // is the baseline for the always-on instrumentation contract.
        let mut gateway = CapabilityGateway::new(Policy::allow_all());
        let mut log = EventLog::new();
        let result = gateway
            .call(&Capability::TimeNow, &[], &mut log)
            .expect("must succeed without a subscriber");
        assert!(matches!(result, Value::Int(_)));
    }

    #[test]
    fn test_wall_time_limit_fires_on_long_running_program() {
        // 0.3-S10: max_wall_ms enforcement.
        // Run a program that loops up to step_limit while a 1ms wall clock
        // applies. The check fires every WALL_TIME_CHECK_EVERY (1024) steps;
        // give the loop room to do many checks. On any modern host, looping
        // ~1M steps takes well over 1ms wall clock.
        let module = simple_module(
            vec![Op::PushConst(0), Op::Pop, Op::Jmp(0)],
            vec![Value::Int(0)],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_max_steps(10_000_000);
        vm.set_max_wall_ms(Some(1));
        let err = vm.run().expect_err("expected WallTimeExceeded");
        match err {
            VmError::WallTimeExceeded(ms) => assert_eq!(ms, 1),
            other => panic!("expected WallTimeExceeded(1), got {other:?}"),
        }
    }

    #[test]
    fn test_wall_time_limit_unset_does_not_fire() {
        // Sanity: a short program runs fine without any wall-clock limit set.
        // Locks the contract that None = no enforcement.
        let module = simple_module(vec![Op::PushConst(0), Op::Ret], vec![Value::Int(42)]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_max_wall_ms(None);
        let result = vm.run().expect("program should run");
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_wall_time_limit_generous_does_not_fire() {
        // Setting an absurdly high wall-clock limit on a fast program should
        // NOT cause spurious WallTimeExceeded errors. Locks against an
        // accidental "always-checking-against-zero" bug.
        let module = simple_module(vec![Op::PushConst(0), Op::Ret], vec![Value::Int(7)]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_max_wall_ms(Some(60_000)); // 60s — surely more than enough
        let result = vm.run().expect("program should complete well under 60s");
        assert_eq!(result, Value::Int(7));
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
            vec![Op::PushConst(0), Op::Dup, Op::EmitUi, Op::Ret],
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
            vec![
                Value::Bool(true),
                Value::String("should not fail".into()),
                Value::Int(42),
            ],
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
            vec![
                Value::Bool(false),
                Value::String("oops".into()),
                Value::Int(42),
            ],
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
        let module = simple_module(vec![Op::PushConst(0), Op::Ret], vec![Value::Int(1)]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.trace_enabled = true;
        vm.run().unwrap();
        assert!(!vm.trace.is_empty());
    }

    // ── List operation tests ──

    #[test]
    fn test_make_list_empty() {
        let module = simple_module(vec![Op::MakeList(0), Op::Ret], vec![]);
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
        let module = simple_module(vec![Op::MakeList(0), Op::ListLen, Op::Ret], vec![]);
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
            vec![Op::PushConst(0), Op::ParseInt, Op::Ret],
            vec![Value::String("42".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_parse_int_negative() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ParseInt, Op::Ret],
            vec![Value::String("-7".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(-7));
    }

    #[test]
    fn test_parse_int_invalid() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ParseInt, Op::Ret],
            vec![Value::String("hello".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_parse_int_empty() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ParseInt, Op::Ret],
            vec![Value::String("".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_parse_int_with_whitespace() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ParseInt, Op::Ret],
            vec![Value::String("  123  ".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(123));
    }

    #[test]
    fn test_try_parse_int_valid() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::TryParseInt, Op::Ret],
            vec![Value::String("42".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::Ok(Box::new(Value::Int(42)))
        );
    }

    #[test]
    fn test_try_parse_int_invalid() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::TryParseInt, Op::Ret],
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
            vec![Op::PushConst(0), Op::TryParseInt, Op::Ret],
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
            vec![
                Value::String("hello world".into()),
                Value::String("world".into()),
            ],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_str_contains_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StrContains, Op::Ret],
            vec![Value::String("hello".into()), Value::String("xyz".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_str_contains_empty_needle() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StrContains, Op::Ret],
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
            vec![
                Value::String("conflict:5".into()),
                Value::String("conflict".into()),
            ],
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
        assert!(
            json.contains("\"version\""),
            "JSON must include version field"
        );
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
        assert!(
            err.contains("unsupported"),
            "error should mention unsupported: {err}"
        );
    }

    #[test]
    fn test_event_log_v1_format_stability() {
        // Golden test: lock the JSON format of EventLog v1
        let mut log = EventLog::new();
        log.log_cap_call(
            &Capability::NetFetch,
            &[Value::String("https://example.com".into())],
        );
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
        assert!(
            json.contains("\"CapResult\""),
            "must have CapResult variant"
        );

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
        let module = simple_module(vec![Op::PushConst(0), Op::Ret], vec![Value::Int(42)]);
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
                Op::PushConst(0), // push 1
                Op::Pop,          // pop it
                Op::Jmp(0),       // loop back to start
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
            vec![Op::PushConst(0), Op::PushConst(1), Op::Add, Op::Ret],
            vec![Value::Int(10), Value::Int(32)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(42));
    }

    #[test]
    fn test_receive_msg_blocks_when_empty() {
        // ReceiveMsg in actor context with empty mailbox returns Blocked.
        // Sprint 0.4-S6: blocking-on-receive is now keyed off
        // `in_actor_context` (set by ActorSystem) instead of
        // `budget.is_some()`, so this test must opt in explicitly.
        let module = simple_module(vec![Op::ReceiveMsg, Op::Ret], vec![]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_in_actor_context(true);
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
        let module = simple_module(vec![Op::ReceiveMsg, Op::Ret], vec![]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.deliver_message(Message {
            from: 99,
            payload: Value::Int(777),
        });
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => assert_eq!(val, Value::Int(777)),
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_msg_legacy_returns_unit() {
        // In legacy unbounded mode (run()), ReceiveMsg pushes Unit
        let module = simple_module(vec![Op::ReceiveMsg, Op::Ret], vec![]);
        assert_eq!(run_module(module).unwrap(), Value::Unit);
    }

    #[test]
    fn test_receive_msg_bounded_standalone_falls_through_with_unit() {
        // Sprint 0.4-S6 contract: a VM driven by `execute_bounded`
        // OUTSIDE an actor system (in_actor_context = false, the
        // default) must mirror legacy `vm.run()` — ReceiveMsg with
        // empty mailbox pushes Unit and execution continues. Without
        // this contract, the streaming-progress path of `boruna_run`
        // would diverge from the non-streaming path for any program
        // that compiles to Op::ReceiveMsg.
        let module = simple_module(vec![Op::ReceiveMsg, Op::Ret], vec![]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        // No set_in_actor_context call — default is false.
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => assert_eq!(val, Value::Unit),
            other => panic!("expected Completed(Unit), got {:?}", other),
        }
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
                Op::SpawnActor(0), // spawn worker -> ActorId(0)
                Op::StoreLocal(0),
                Op::SpawnActor(0), // spawn worker -> ActorId(1)
                Op::StoreLocal(1),
                Op::LoadLocal(0), // push first
                Op::LoadLocal(1), // push second
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
                Op::SpawnActor(0), // spawn worker
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
                Op::PushConst(0), // target: ActorId(5)
                Op::PushConst(1), // payload: Int(42)
                Op::SendMsg,
                Op::PushConst(2), // return 0
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
                Op::PushConst(0), // target: Int(5) -- not ActorId!
                Op::PushConst(1), // payload
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
                Op::ReceiveMsg, // first message
                Op::StoreLocal(0),
                Op::ReceiveMsg, // second message
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
        vm.deliver_message(Message {
            from: 0,
            payload: Value::Int(111),
        });
        vm.deliver_message(Message {
            from: 0,
            payload: Value::Int(222),
        });
        vm.set_entry_function(0).unwrap();
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Completed(val) => match val {
                Value::Record { fields, .. } => {
                    assert_eq!(fields[0], Value::Int(111));
                    assert_eq!(fields[1], Value::Int(222));
                }
                _ => panic!("expected Record"),
            },
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_receive_blocks_then_resumes() {
        // Actor blocks on receive, then resumes after message delivery.
        // Sprint 0.4-S6: requires explicit actor-context flag.
        use crate::actor::Message;
        let module = simple_module(
            vec![
                Op::ReceiveMsg, // will block first time
                Op::Ret,
            ],
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.set_in_actor_context(true);
        vm.set_entry_function(0).unwrap();

        // First attempt: blocks
        match vm.execute_bounded(100) {
            crate::vm::StepResult::Blocked => {} // expected
            other => panic!("expected Blocked, got {:?}", other),
        }

        // Deliver a message
        vm.deliver_message(Message {
            from: 0,
            payload: Value::Int(999),
        });

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
                Op::ReceiveMsg, // blocks until message arrives
                Op::Ret,        // return payload
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
                Op::SpawnActor(0), // spawn worker -> ActorId
                Op::StoreLocal(0), // save child id
                Op::LoadLocal(0),  // push target
                Op::PushConst(0),  // payload: Int(77)
                Op::SendMsg,       // send to child
                Op::PushConst(1),  // return 0
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
                Op::PushConst(0), // target: ActorId(0) = parent
                Op::PushConst(1), // payload: Int(55)
                Op::SendMsg,
                Op::PushConst(2), // return 0
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
                Op::SpawnActor(0), // spawn worker
                Op::Pop,           // discard child ActorId
                Op::ReceiveMsg,    // blocks until child's message arrives
                Op::Ret,           // return received payload
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
            code: vec![Op::SpawnActor(0), Op::Pop, Op::ReceiveMsg, Op::Ret],
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
                Op::ReceiveMsg,   // 0: receive
                Op::PushConst(0), // 1: target = ActorId(0) (parent)
                Op::PushConst(1), // 2: payload = Int(1)
                Op::SendMsg,      // 3: send
                Op::Jmp(0),       // 4: loop back
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
                Op::SpawnActor(0), // 0: spawn worker
                Op::StoreLocal(0), // 1: save child id
                Op::LoadLocal(0),  // 2: push child id
                Op::PushConst(1),  // 3: payload = Int(1)
                Op::SendMsg,       // 4: send initial message
                Op::ReceiveMsg,    // 5: receive reply
                Op::LoadLocal(0),  // 6: push child id
                Op::PushConst(1),  // 7: payload
                Op::SendMsg,       // 8: send again
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
        system
            .send(
                0,
                crate::actor::Message {
                    from: 99,
                    payload: Value::Int(123),
                },
            )
            .unwrap();
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
                Op::PushConst(0), // ActorId(0)
                Op::PushConst(1), // Int(1)
                Op::SendMsg,
                Op::PushConst(3), // return 0
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
                Op::PushConst(0), // ActorId(0)
                Op::PushConst(2), // Int(2)
                Op::SendMsg,
                Op::PushConst(3), // return 0
                Op::Ret,
            ],
            capabilities: vec![],
            match_tables: vec![],
        });
        module.constants.push(Value::ActorId(0)); // 0: parent id
        module.constants.push(Value::Int(1)); // 1
        module.constants.push(Value::Int(2)); // 2
        module.constants.push(Value::Int(0)); // 3
                                              // Function 2: main — spawns both workers, receives two messages
                                              // Returns a list [first_msg, second_msg]
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 2,
            code: vec![
                Op::SpawnActor(0), // spawn worker_a
                Op::Pop,
                Op::SpawnActor(1), // spawn worker_b
                Op::Pop,
                Op::ReceiveMsg, // first message
                Op::StoreLocal(0),
                Op::ReceiveMsg, // second message
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
            Event::SchedulerTick {
                round,
                active_actor,
            } => {
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
            ReplayResult::Diverged { reason } => {
                panic!("expected Identical, got Diverged: {}", reason)
            }
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
                Op::PushConst(0), // ActorId(0)
                Op::PushConst(1), // Int(99)
                Op::SendMsg,
                Op::PushConst(2), // return 0
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
            code: vec![Op::SpawnActor(0), Op::Pop, Op::ReceiveMsg, Op::Ret],
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
        let has_scheduler_tick = events
            .iter()
            .any(|e| matches!(e, Event::SchedulerTick { .. }));
        let has_actor_spawn = events.iter().any(|e| matches!(e, Event::ActorSpawn { .. }));
        let has_message_send = events
            .iter()
            .any(|e| matches!(e, Event::MessageSend { .. }));
        let has_message_receive = events
            .iter()
            .any(|e| matches!(e, Event::MessageReceive { .. }));
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
                Op::ReceiveMsg, // receives error from crashed child
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
            Value::Err(inner) => match *inner {
                Value::String(s) => assert!(
                    s.contains("division by zero"),
                    "expected division by zero error, got: {s}"
                ),
                other => panic!("expected Err(String), got Err({:?})", other),
            },
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
                Op::PushConst(0), // 1
                Op::PushConst(1), // 0
                Op::Div,          // crash!
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
                Op::ReceiveMsg, // receives error from crashed child
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
            Value::Err(inner) => match *inner {
                Value::String(s) => assert!(
                    s.contains("division by zero"),
                    "expected division by zero, got: {s}"
                ),
                other => panic!("expected Err(String), got Err({:?})", other),
            },
            other => panic!("expected Err from cascade, got {:?}", other),
        }
        // 2 actors total: root(0), child(1). Grandchild was never spawned
        // because child crashed — spawn requests from failed actors are discarded.
        assert_eq!(system.actor_count(), 2);
    }

    #[test]
    fn test_deadlock_error_display() {
        let err = VmError::Deadlock;
        assert_eq!(
            format!("{err}"),
            "deadlock: all actors blocked with no pending messages"
        );
    }

    #[test]
    fn test_max_rounds_error_display() {
        let err = VmError::MaxRoundsExceeded(100);
        assert_eq!(format!("{err}"), "max scheduler rounds exceeded (100)");
    }

    // ── T-2.2: last_cap_events tracking ──

    #[test]
    fn test_take_last_cap_events_records_cap_name() {
        // A one-shot CapCall (time.now, id=5) followed by Ret.
        // After vm.run(), last_cap_events should have been populated
        // and be drainable via take_last_cap_events.
        let module = simple_module(
            vec![Op::CapCall(5, 0), Op::Ret], // time.now, no args
            vec![],
        );
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.run().expect("should succeed");
        let events = vm.take_last_cap_events();
        assert_eq!(events, vec!["time.now"], "cap name must be recorded");
    }

    #[test]
    fn test_take_last_cap_events_clears_on_drain() {
        let module = simple_module(vec![Op::CapCall(5, 0), Op::Ret], vec![]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.run().expect("should succeed");
        let _ = vm.take_last_cap_events();
        assert!(
            vm.take_last_cap_events().is_empty(),
            "second drain must return empty"
        );
    }

    #[test]
    fn test_take_last_cap_events_empty_for_pure_program() {
        let module = simple_module(vec![Op::PushConst(0), Op::Ret], vec![Value::Int(42)]);
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.run().expect("should succeed");
        assert!(
            vm.take_last_cap_events().is_empty(),
            "pure program must leave cap events empty"
        );
    }

    // ── New string built-in tests ──

    #[test]
    fn test_int_to_string() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::IntToString, Op::Ret],
            vec![Value::Int(42)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("42".into())
        );
    }

    #[test]
    fn test_int_to_string_negative() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::IntToString, Op::Ret],
            vec![Value::Int(-7)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("-7".into())
        );
    }

    #[test]
    fn test_float_to_string() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::FloatToString, Op::Ret],
            vec![Value::Float(3.14)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("3.14".into())
        );
    }

    #[test]
    fn test_string_len() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringLen, Op::Ret],
            vec![Value::String("hello".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(5));
    }

    #[test]
    fn test_string_len_empty() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringLen, Op::Ret],
            vec![Value::String("".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(0));
    }

    #[test]
    fn test_string_chars() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringChars, Op::Ret],
            vec![Value::String("ab".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![
                Value::String("a".into()),
                Value::String("b".into()),
            ])
        );
    }

    #[test]
    fn test_string_chars_empty() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringChars, Op::Ret],
            vec![Value::String("".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::List(vec![]));
    }

    // ── New string built-ins (post1/more-string-list-builtins) ──

    #[test]
    fn test_string_contains_true() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringContains, Op::Ret],
            vec![Value::String("hello world".into()), Value::String("world".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_string_contains_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringContains, Op::Ret],
            vec![Value::String("hello".into()), Value::String("xyz".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_string_starts_with_true() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringStartsWith, Op::Ret],
            vec![Value::String("hello world".into()), Value::String("hello".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_string_starts_with_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringStartsWith, Op::Ret],
            vec![Value::String("hello".into()), Value::String("world".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_string_ends_with_true() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringEndsWith, Op::Ret],
            vec![Value::String("hello world".into()), Value::String("world".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_string_ends_with_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringEndsWith, Op::Ret],
            vec![Value::String("hello".into()), Value::String("xyz".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_string_to_upper() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringToUpper, Op::Ret],
            vec![Value::String("hello".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("HELLO".into()));
    }

    #[test]
    fn test_string_to_lower() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringToLower, Op::Ret],
            vec![Value::String("HELLO".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("hello".into()));
    }

    #[test]
    fn test_string_trim() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::StringTrim, Op::Ret],
            vec![Value::String("  hello  ".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("hello".into()));
    }

    #[test]
    fn test_string_join() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringJoin, Op::Ret],
            vec![
                Value::List(vec![
                    Value::String("a".into()),
                    Value::String("b".into()),
                    Value::String("c".into()),
                ]),
                Value::String(", ".into()),
            ],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("a, b, c".into()));
    }

    #[test]
    fn test_string_join_empty_list() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringJoin, Op::Ret],
            vec![Value::List(vec![]), Value::String(",".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("".into()));
    }

    // ── New list built-ins ──

    #[test]
    fn test_list_len_builtin() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListLenBuiltin, Op::Ret],
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
            ])],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(3));
    }

    #[test]
    fn test_list_is_empty_true() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListIsEmpty, Op::Ret],
            vec![Value::List(vec![])],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_list_is_empty_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListIsEmpty, Op::Ret],
            vec![Value::List(vec![Value::Int(1)])],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_list_head_some() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListHead, Op::Ret],
            vec![Value::List(vec![Value::Int(10), Value::Int(20)])],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::Some(Box::new(Value::Int(10)))
        );
    }

    #[test]
    fn test_list_head_none() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListHead, Op::Ret],
            vec![Value::List(vec![])],
        );
        assert_eq!(run_module(module).unwrap(), Value::None);
    }

    #[test]
    fn test_list_tail() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListTail, Op::Ret],
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
            ])],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn test_list_tail_empty() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListTail, Op::Ret],
            vec![Value::List(vec![])],
        );
        assert_eq!(run_module(module).unwrap(), Value::List(vec![]));
    }

    #[test]
    fn test_list_append() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::ListAppend, Op::Ret],
            vec![
                Value::List(vec![Value::Int(1), Value::Int(2)]),
                Value::Int(3),
            ],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn test_list_concat() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::ListConcat, Op::Ret],
            vec![
                Value::List(vec![Value::Int(1), Value::Int(2)]),
                Value::List(vec![Value::Int(3), Value::Int(4)]),
            ],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
            ])
        );
    }

    #[test]
    fn test_list_reverse() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::ListReverse, Op::Ret],
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
            ])],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
        );
    }

    // ── New string/map built-ins ──

    #[test]
    fn test_string_split() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringSplit, Op::Ret],
            vec![Value::String("a,b,c".into()), Value::String(",".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
            ])
        );
    }

    #[test]
    fn test_string_split_empty_sep() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::StringSplit, Op::Ret],
            vec![Value::String("ab".into()), Value::String("".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![
                Value::String("a".into()),
                Value::String("b".into()),
            ])
        );
    }

    #[test]
    fn test_string_replace() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::PushConst(2),
                Op::StringReplace,
                Op::Ret,
            ],
            vec![
                Value::String("hello world".into()),
                Value::String("world".into()),
                Value::String("Rust".into()),
            ],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("hello Rust".into())
        );
    }

    #[test]
    fn test_string_slice() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::PushConst(2),
                Op::StringSlice,
                Op::Ret,
            ],
            vec![
                Value::String("hello".into()),
                Value::Int(1),
                Value::Int(4),
            ],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::String("ell".into())
        );
    }

    #[test]
    fn test_string_slice_out_of_bounds() {
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::PushConst(2),
                Op::StringSlice,
                Op::Ret,
            ],
            vec![
                Value::String("hi".into()),
                Value::Int(0),
                Value::Int(100),
            ],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("".into()));
    }

    #[test]
    fn test_int_parse_ok() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::IntParse, Op::Ret],
            vec![Value::String("42".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::Some(Box::new(Value::Int(42)))
        );
    }

    #[test]
    fn test_int_parse_fail() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::IntParse, Op::Ret],
            vec![Value::String("abc".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::None);
    }

    #[test]
    fn test_float_parse_ok() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::FloatParse, Op::Ret],
            vec![Value::String("3.14".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::Some(Box::new(Value::Float(3.14)))
        );
    }

    #[test]
    fn test_float_parse_fail() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::FloatParse, Op::Ret],
            vec![Value::String("nope".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::None);
    }

    #[test]
    fn test_bool_to_string_true() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::BoolToString, Op::Ret],
            vec![Value::Bool(true)],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("true".into()));
    }

    #[test]
    fn test_bool_to_string_false() {
        let module = simple_module(
            vec![Op::PushConst(0), Op::BoolToString, Op::Ret],
            vec![Value::Bool(false)],
        );
        assert_eq!(run_module(module).unwrap(), Value::String("false".into()));
    }

    #[test]
    fn test_map_get_some() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("key".to_string(), Value::Int(99));
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::MapGet, Op::Ret],
            vec![Value::Map(m), Value::String("key".into())],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::Some(Box::new(Value::Int(99)))
        );
    }

    #[test]
    fn test_map_get_none() {
        use std::collections::BTreeMap;
        let m: BTreeMap<String, Value> = BTreeMap::new();
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::MapGet, Op::Ret],
            vec![Value::Map(m), Value::String("missing".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::None);
    }

    #[test]
    fn test_map_set() {
        use std::collections::BTreeMap;
        let m: BTreeMap<String, Value> = BTreeMap::new();
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::PushConst(2),
                Op::MapSet,
                Op::Ret,
            ],
            vec![
                Value::Map(m),
                Value::String("x".into()),
                Value::Int(5),
            ],
        );
        let mut expected = BTreeMap::new();
        expected.insert("x".to_string(), Value::Int(5));
        assert_eq!(run_module(module).unwrap(), Value::Map(expected));
    }

    #[test]
    fn test_map_remove() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), Value::Int(1));
        m.insert("b".to_string(), Value::Int(2));
        let module = simple_module(
            vec![Op::PushConst(0), Op::PushConst(1), Op::MapRemove, Op::Ret],
            vec![Value::Map(m), Value::String("a".into())],
        );
        let mut expected = BTreeMap::new();
        expected.insert("b".to_string(), Value::Int(2));
        assert_eq!(run_module(module).unwrap(), Value::Map(expected));
    }

    #[test]
    fn test_map_contains_key_true() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("k".to_string(), Value::Bool(true));
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::MapContainsKey,
                Op::Ret,
            ],
            vec![Value::Map(m), Value::String("k".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_map_contains_key_false() {
        use std::collections::BTreeMap;
        let m: BTreeMap<String, Value> = BTreeMap::new();
        let module = simple_module(
            vec![
                Op::PushConst(0),
                Op::PushConst(1),
                Op::MapContainsKey,
                Op::Ret,
            ],
            vec![Value::Map(m), Value::String("missing".into())],
        );
        assert_eq!(run_module(module).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_map_keys() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), Value::Int(1));
        m.insert("b".to_string(), Value::Int(2));
        let module = simple_module(
            vec![Op::PushConst(0), Op::MapKeys, Op::Ret],
            vec![Value::Map(m)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::String("a".into()), Value::String("b".into())])
        );
    }

    #[test]
    fn test_map_values() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), Value::Int(10));
        m.insert("b".to_string(), Value::Int(20));
        let module = simple_module(
            vec![Op::PushConst(0), Op::MapValues, Op::Ret],
            vec![Value::Map(m)],
        );
        assert_eq!(
            run_module(module).unwrap(),
            Value::List(vec![Value::Int(10), Value::Int(20)])
        );
    }

    #[test]
    fn test_map_len() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("x".to_string(), Value::Unit);
        m.insert("y".to_string(), Value::Unit);
        m.insert("z".to_string(), Value::Unit);
        let module = simple_module(
            vec![Op::PushConst(0), Op::MapLen, Op::Ret],
            vec![Value::Map(m)],
        );
        assert_eq!(run_module(module).unwrap(), Value::Int(3));
    }
}
