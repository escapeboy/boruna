use boruna_bytecode::Value;

/// A declarative side effect produced by update().
///
/// Effects are not executed inside update() — they are returned as data
/// and the framework runtime executes them via the capability gateway.
#[derive(Debug, Clone)]
pub struct Effect {
    pub kind: EffectKind,
    pub payload: Value,
    /// The message variant to deliver with the effect result.
    pub callback_tag: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EffectKind {
    HttpRequest,
    DbQuery,
    FsRead,
    FsWrite,
    Timer,
    Random,
    SpawnActor,
    EmitUi,
    LlmCall,
    SendToActor,
}

impl EffectKind {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "http_request" => Some(EffectKind::HttpRequest),
            "db_query" => Some(EffectKind::DbQuery),
            "fs_read" => Some(EffectKind::FsRead),
            "fs_write" => Some(EffectKind::FsWrite),
            "timer" => Some(EffectKind::Timer),
            "random" => Some(EffectKind::Random),
            "spawn_actor" => Some(EffectKind::SpawnActor),
            "emit_ui" => Some(EffectKind::EmitUi),
            "llm_call" => Some(EffectKind::LlmCall),
            "send_to_actor" => Some(EffectKind::SendToActor),
            _ => None,
        }
    }

    pub fn capability_name(&self) -> &'static str {
        match self {
            EffectKind::HttpRequest => "net.fetch",
            EffectKind::DbQuery => "db.query",
            EffectKind::FsRead => "fs.read",
            EffectKind::FsWrite => "fs.write",
            EffectKind::Timer => "time.now",
            EffectKind::Random => "random",
            EffectKind::SpawnActor => "actor.spawn",
            EffectKind::EmitUi => "ui.render",
            EffectKind::LlmCall => "llm.call",
            EffectKind::SendToActor => "actor.send",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EffectKind::HttpRequest => "http_request",
            EffectKind::DbQuery => "db_query",
            EffectKind::FsRead => "fs_read",
            EffectKind::FsWrite => "fs_write",
            EffectKind::Timer => "timer",
            EffectKind::Random => "random",
            EffectKind::SpawnActor => "spawn_actor",
            EffectKind::EmitUi => "emit_ui",
            EffectKind::LlmCall => "llm_call",
            EffectKind::SendToActor => "send_to_actor",
        }
    }
}

/// Extract items from a value that may be a List or a Record{type_id:0xFFFF} (list literal).
fn as_list(value: &Value) -> Option<&[Value]> {
    match value {
        Value::List(items) => Some(items),
        // List literals compile to Record with type_id 0xFFFF
        Value::Record {
            type_id, fields, ..
        } if *type_id == 0xFFFF => Some(fields),
        _ => None,
    }
}

/// Parse effects from the VM's return value of update().
///
/// update() returns a Record with fields: [state, effects_list]
/// effects_list is a List of Records with fields: [kind, payload, callback_tag]
pub fn parse_effects(effects_value: &Value) -> Vec<Effect> {
    let items = match as_list(effects_value) {
        Some(items) => items,
        None => return Vec::new(),
    };

    items
        .iter()
        .filter_map(|item| match item {
            Value::Record { fields, .. } if fields.len() >= 3 => {
                let kind_str = match &fields[0] {
                    Value::String(s) => s.as_str(),
                    _ => return None,
                };
                let kind = EffectKind::parse_str(kind_str)?;
                let payload = fields[1].clone();
                let callback_tag = match &fields[2] {
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                };
                Some(Effect {
                    kind,
                    payload,
                    callback_tag,
                })
            }
            _ => None,
        })
        .collect()
}

/// Parse the UpdateResult from the VM return value.
///
/// UpdateResult is a Record with fields: [state, effects]
pub fn parse_update_result(value: &Value) -> Option<(Value, Vec<Effect>)> {
    match value {
        Value::Record { fields, .. } if fields.len() >= 2 => {
            let state = fields[0].clone();
            let effects = parse_effects(&fields[1]);
            Some((state, effects))
        }
        _ => None,
    }
}
