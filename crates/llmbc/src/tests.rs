#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::module::*;
    use crate::*;

    /// Locked by `docs/spec/bytecode-1.0.md` (sprint `W9-A`). The
    /// `BYTECODE_VERSION` constant is the public spec identifier; bumping it
    /// is a coordinated spec freeze, not a routine release operation.
    ///
    /// **1.1** added `Op::Debug` (0xA7) and `Op::DebugMsg` (0xA8) per
    /// §1.2(6) of the spec — additive opcode minor bump.
    #[test]
    fn test_bytecode_version_is_1_1() {
        assert_eq!(BYTECODE_VERSION, "1.1");
    }

    /// The new 1.1 opcodes must have stable byte tags that do not collide
    /// with any existing assignment. A regression here means the 1.x line
    /// has been corrupted.
    #[test]
    fn test_bytecode_1_1_debug_opcodes_have_assigned_tags() {
        assert_eq!(Op::Debug.to_byte_tag(), 0xA7);
        assert_eq!(Op::DebugMsg.to_byte_tag(), 0xA8);
    }

    /// Asserts the 1.1 additions do not collide with any other opcode tag.
    /// Catches accidental reuse of a discriminant under refactoring.
    #[test]
    fn test_bytecode_1_1_debug_opcodes_do_not_alias() {
        let all_tags = [
            Op::PushConst(0).to_byte_tag(),
            Op::LoadLocal(0).to_byte_tag(),
            Op::Call(0, 0).to_byte_tag(),
            Op::Ret.to_byte_tag(),
            Op::MapLen.to_byte_tag(),
            Op::Debug.to_byte_tag(),
            Op::DebugMsg.to_byte_tag(),
            Op::Nop.to_byte_tag(),
            Op::Halt.to_byte_tag(),
        ];
        for (i, t1) in all_tags.iter().enumerate() {
            for (j, t2) in all_tags.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        t1, t2,
                        "tag collision between index {i} and {j}: 0x{t1:02X}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_module_json_roundtrip() {
        let mut module = Module::new("test");
        module.add_const(Value::Int(42));
        module.add_const(Value::String("hello".into()));
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 1,
            code: vec![Op::PushConst(0), Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });

        let json = module.to_json().unwrap();
        let restored = Module::from_json(&json).unwrap();
        assert_eq!(module, restored);
    }

    #[test]
    fn test_module_binary_roundtrip() {
        let mut module = Module::new("test");
        module.add_const(Value::Int(100));
        module.add_const(Value::Bool(true));
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::PushConst(0), Op::PushConst(1), Op::Halt],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });

        let bytes = module.to_bytes().unwrap();
        assert_eq!(&bytes[0..4], &MAGIC);
        let restored = Module::from_bytes(&bytes).unwrap();
        assert_eq!(module, restored);
    }

    #[test]
    fn test_module_intent_json_roundtrip() {
        let mut module = Module::new("test");
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Ret],
            capabilities: vec![],
            intent: Some("Declared purpose of this step".into()),
            match_tables: vec![],
        });
        let json = module.to_json().unwrap();
        let restored = Module::from_json(&json).unwrap();
        assert_eq!(module, restored);
        assert_eq!(
            restored.functions[0].intent.as_deref(),
            Some("Declared purpose of this step")
        );
    }

    #[test]
    fn test_transitively_invokes_model() {
        let mut module = Module::new("t");
        // idx 0: helper declaring llm.call
        module.add_function(Function {
            name: "helper".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Ret],
            capabilities: vec![Capability::LlmCall],
            intent: None,
            match_tables: vec![],
        });
        // idx 1: main calls helper (transitively reaches the model)
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Call(0, 0), Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        // idx 2: pure function, neither declares nor reaches llm.call
        module.add_function(Function {
            name: "pure".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        assert!(module.transitively_invokes(1, Capability::LlmCall));
        assert!(module.transitively_invokes(0, Capability::LlmCall));
        assert!(!module.transitively_invokes(2, Capability::LlmCall));
    }

    #[test]
    fn test_transitively_invokes_is_cycle_safe() {
        // Mutually recursive functions must not loop forever.
        let mut module = Module::new("t");
        module.add_function(Function {
            name: "a".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Call(1, 0), Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        module.add_function(Function {
            name: "b".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Call(0, 0), Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        assert!(!module.transitively_invokes(0, Capability::LlmCall));
    }

    #[test]
    fn test_needed_and_over_declared_capabilities() {
        let mut module = Module::new("t");
        // idx 0: worker directly does a NetFetch effect and declares exactly it.
        module.add_function(Function {
            name: "worker".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::CapCall(Capability::NetFetch.id(), 0), Op::Ret],
            capabilities: vec![Capability::NetFetch],
            intent: None,
            match_tables: vec![],
        });
        // idx 1: `over` only calls worker but declares NetFetch AND FsWrite.
        // It transitively needs NetFetch (via worker) but never needs FsWrite.
        module.add_function(Function {
            name: "over".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Call(0, 0), Op::Ret],
            capabilities: vec![Capability::NetFetch, Capability::FsWrite],
            intent: None,
            match_tables: vec![],
        });
        // worker: minimal — no over-grant.
        assert_eq!(module.needed_capabilities(0), vec![Capability::NetFetch]);
        assert!(module.over_declared_capabilities(0).is_empty());
        // over: needs NetFetch transitively; FsWrite is an over-grant.
        assert_eq!(module.needed_capabilities(1), vec![Capability::NetFetch]);
        assert_eq!(
            module.over_declared_capabilities(1),
            vec![Capability::FsWrite]
        );
    }

    #[test]
    fn test_needed_capabilities_cycle_safe() {
        let mut module = Module::new("t");
        module.add_function(Function {
            name: "a".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Call(1, 0), Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        module.add_function(Function {
            name: "b".into(),
            arity: 0,
            locals: 0,
            code: vec![
                Op::CapCall(Capability::DbQuery.id(), 0),
                Op::Call(0, 0),
                Op::Ret,
            ],
            capabilities: vec![Capability::DbQuery],
            intent: None,
            match_tables: vec![],
        });
        // a → b → a (cycle); a transitively needs DbQuery via b, no infinite loop.
        assert_eq!(module.needed_capabilities(0), vec![Capability::DbQuery]);
    }

    #[test]
    fn test_module_legacy_json_without_intent_defaults_to_none() {
        // A module serialized before Sprint 1 has no `intent` key on its
        // functions. `#[serde(default)]` must load it cleanly as `None`.
        let mut module = Module::new("test");
        module.add_function(Function {
            name: "main".into(),
            arity: 0,
            locals: 0,
            code: vec![Op::Ret],
            capabilities: vec![],
            intent: None,
            match_tables: vec![],
        });
        let json = module.to_json().unwrap();
        // Strip the `intent` key from every function object to simulate a
        // pre-Sprint-1 serialized module.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        for func in value["functions"].as_array_mut().unwrap() {
            func.as_object_mut().unwrap().remove("intent");
        }
        let legacy_json = serde_json::to_string(&value).unwrap();
        assert!(!legacy_json.contains("intent"));
        let restored = Module::from_json(&legacy_json).unwrap();
        assert_eq!(restored.functions[0].intent, None);
    }

    #[test]
    fn test_invalid_magic() {
        let data = vec![
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x00, 0x00, b'{', b'}',
        ];
        assert!(Module::from_bytes(&data).is_err());
    }

    #[test]
    fn test_value_display() {
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::String("hi".into())), "\"hi\"");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::None), "None");
        assert_eq!(
            format!("{}", Value::Some(Box::new(Value::Int(1)))),
            "Some(1)"
        );
    }

    #[test]
    fn test_value_truthiness() {
        assert!(!Value::Unit.is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::String("x".into()).is_truthy());
        assert!(!Value::String(String::new()).is_truthy());
        assert!(!Value::None.is_truthy());
        assert!(Value::Some(Box::new(Value::Unit)).is_truthy());
    }

    #[test]
    fn test_capability_roundtrip() {
        for cap in &[
            Capability::NetFetch,
            Capability::FsRead,
            Capability::FsWrite,
            Capability::DbQuery,
            Capability::UiRender,
            Capability::TimeNow,
            Capability::Random,
        ] {
            let id = cap.id();
            let restored = Capability::from_id(id).unwrap();
            assert_eq!(cap, &restored);
            let name = cap.name();
            let from_name = Capability::from_name(name).unwrap();
            assert_eq!(cap, &from_name);
        }
    }

    // ── 0.3-S11: capability_set_hash tests ──

    #[test]
    fn test_capability_all_is_sorted_by_name() {
        let names: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "Capability::ALL must be sorted ascending by name() — \
             this lock-step ordering is what makes capability_set_hash stable across builds"
        );
    }

    #[test]
    fn test_capability_all_has_no_duplicates() {
        let mut names: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(original_len, names.len(), "Capability::ALL has a duplicate");
    }

    #[test]
    fn test_capability_all_covers_every_variant() {
        // Self-extending: walk from id=0 until from_id returns None. Locks the
        // invariant "every variant the bytecode can construct must appear in
        // Capability::ALL" without hardcoding the variant count.
        let mut id = 0u32;
        while let Some(cap) = Capability::from_id(id) {
            assert!(
                Capability::ALL.contains(&cap),
                "Capability::{cap:?} (id={id}) is not in Capability::ALL — \
                 forgot to add it after introducing a new capability?"
            );
            id += 1;
        }
        // Sanity check: also lock the count, so an accidental ALL trim is loud.
        assert_eq!(
            Capability::ALL.len(),
            id as usize,
            "Capability::ALL length ({}) does not match the number of variants \
             reachable via from_id (0..{id}). Either ALL is missing a variant \
             or has an extra one.",
            Capability::ALL.len()
        );
    }

    #[test]
    fn test_capability_version_is_one_for_all_shipped() {
        for cap in Capability::ALL.iter() {
            assert_eq!(
                cap.version(),
                "1",
                "Capability::{cap:?} has version != \"1\". \
                 If you bumped a capability version, also update \
                 test_capability_set_hash_known_value's golden hash and CHANGELOG."
            );
        }
    }

    #[test]
    fn test_capability_set_hash_is_deterministic() {
        let r1 = capability_set_report("boruna", "0.2.0");
        let r2 = capability_set_report("boruna", "0.2.0");
        assert_eq!(r1.capability_set_hash, r2.capability_set_hash);
        // Sanity: hash is not empty / sentinel
        assert!(r1.capability_set_hash.starts_with("sha256:"));
        assert_eq!(r1.capability_set_hash.len(), 7 + 64);
    }

    #[test]
    fn test_capability_set_hash_known_value() {
        // Golden hash for the current capability surface, computed externally
        // with `shasum -a 256` over the documented canonical encoding (see
        // docs/reference/capability-identity.md).
        //
        // If this test fails, EITHER:
        //   (a) you intentionally changed the capability set / a version → bump
        //       the golden value here and add a CHANGELOG entry, OR
        //   (b) you accidentally changed the hash algorithm — restore it. The
        //       algorithm itself is also locked by
        //       test_compute_capability_set_hash_algorithm_known_value below.
        let report = capability_set_report("boruna", "0.2.0");
        assert_eq!(
            report.capability_set_hash,
            // Bumped in 0.3-S14 for the new step.input capability.
            // Computed externally:
            //   printf 'actor.send\t1\nactor.spawn\t1\ndb.query\t1\nfs.read\t1\nfs.write\t1\nllm.call\t1\nnet.fetch\t1\nrandom\t1\nstep.input\t1\ntime.now\t1\nui.render\t1\n' | shasum -a 256
            // Integrators using the prior hash for cache keys MUST
            // invalidate — additive surface change per the documented
            // contract.
            "sha256:980d017dc54e30c39c329484b501fbe46914d9ad344bfcb4610b0280f4300a67"
        );
    }

    #[test]
    fn test_compute_capability_set_hash_algorithm_known_value() {
        // Locks the BYTE-STRING ENCODING RULE (not just the current capability
        // surface): for input [("a","1"), ("b","2")], the canonical encoding is
        // exactly "a\t1\nb\t2\n" UTF-8 → SHA-256.
        //
        // Reproduce: `printf 'a\t1\nb\t2\n' | shasum -a 256` →
        //   6d2d1bd0abaed39e891321f7fb19d3f21108674b420432e927ae2fb4d0b7fb73
        //
        // If this test fails but test_capability_set_hash_known_value is also
        // updated, the algorithm has silently drifted from the documented
        // contract — every external reproducer using the docs' shasum recipe
        // will now disagree with the binary. Restore the algorithm.
        let hash = compute_capability_set_hash([("a", "1"), ("b", "2")]);
        assert_eq!(
            hash,
            "sha256:6d2d1bd0abaed39e891321f7fb19d3f21108674b420432e927ae2fb4d0b7fb73"
        );
    }

    #[test]
    fn test_capability_set_hash_ignores_name_and_version() {
        // The hash covers (cap_name, cap_version) pairs ONLY — never the
        // binary name or binary version. This is the entire point: integrators
        // can cache results across binary upgrades that touch neither the
        // capability set nor any capability's contract.
        let r1 = capability_set_report("boruna", "0.2.0");
        let r2 = capability_set_report("boruna", "99.9.9");
        let r3 = capability_set_report("fork-of-boruna", "0.2.0");
        assert_eq!(r1.capability_set_hash, r2.capability_set_hash);
        assert_eq!(r1.capability_set_hash, r3.capability_set_hash);
    }

    #[test]
    fn test_capability_set_report_shape() {
        let report = capability_set_report("boruna", "0.2.0");
        assert_eq!(
            report.protocol_version, CAPABILITY_REPORT_PROTOCOL_VERSION,
            "report must advertise its wire-format version"
        );
        assert_eq!(report.name, "boruna");
        assert_eq!(report.version, "0.2.0");
        assert_eq!(report.capabilities.len(), 11);
        for ident in &report.capabilities {
            assert!(!ident.name.is_empty());
            assert!(!ident.version.is_empty());
        }
        // Same ordering as Capability::ALL
        let expected: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
        let actual: Vec<&str> = report
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_capability_set_report_accepts_fork_branding() {
        // A downstream fork can rebrand without patching this crate.
        let report = capability_set_report("acme-flow", "1.2.3");
        assert_eq!(report.name, "acme-flow");
        assert_eq!(report.version, "1.2.3");
    }

    #[test]
    fn test_capability_set_hash_changes_on_version_bump() {
        let baseline = compute_capability_set_hash(
            Capability::ALL
                .iter()
                .map(|c| (c.name().to_string(), c.version().to_string())),
        );
        // Simulate bumping net.fetch from "1" to "2"
        let bumped = compute_capability_set_hash(Capability::ALL.iter().map(|c| {
            let v = if matches!(c, Capability::NetFetch) {
                "2"
            } else {
                c.version()
            };
            (c.name().to_string(), v.to_string())
        }));
        assert_ne!(
            baseline, bumped,
            "bumping a capability version must change the set hash"
        );
    }
}
