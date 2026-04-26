#[cfg(test)]
mod tests {
    use crate::module::*;
    use crate::*;

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
            match_tables: vec![],
        });

        let bytes = module.to_bytes().unwrap();
        assert_eq!(&bytes[0..4], &MAGIC);
        let restored = Module::from_bytes(&bytes).unwrap();
        assert_eq!(module, restored);
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
                Capability::ALL.iter().any(|c| *c == cap),
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
