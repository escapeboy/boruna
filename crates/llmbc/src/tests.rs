#[cfg(test)]
mod tests {
    use crate::*;
    use crate::module::*;

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
        let data = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x00, 0x00, b'{', b'}'];
        assert!(Module::from_bytes(&data).is_err());
    }

    #[test]
    fn test_value_display() {
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::String("hi".into())), "\"hi\"");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::None), "None");
        assert_eq!(format!("{}", Value::Some(Box::new(Value::Int(1)))), "Some(1)");
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
}
