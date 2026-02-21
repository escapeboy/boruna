# Framework Public API (v0.1.0)

Stability: **0.x** — breaking changes allowed between minor versions.
All unlisted types/functions are internal and may change without notice.

## boruna_framework (crate root re-exports)

```rust
pub use error::FrameworkError;
pub use runtime::AppRuntime;
pub use validate::AppValidator;
pub use testing::TestHarness;
pub use policy::PolicySet;
```

## boruna_framework::error

```rust
pub enum FrameworkError {
    Validation(String),
    MissingFunction(String),
    PurityViolation { name: String },
    WrongArity { name: String, expected: usize, got: usize },
    MissingType(String),
    Effect(String),
    PolicyViolation(String),
    State(String),
    Compile(boruna_compiler::CompileError),
    Runtime(boruna_vm::VmError),
    MaxCyclesExceeded(u64),
}
```

## boruna_framework::validate

```rust
pub struct AppValidator;

impl AppValidator {
    pub fn validate(program: &Program) -> Result<ValidationResult, FrameworkError>;
    pub fn is_valid_app(program: &Program) -> bool;
}

pub struct ValidationResult {
    pub has_init: bool,
    pub has_update: bool,
    pub has_view: bool,
    pub has_policies: bool,
    pub state_type: Option<String>,
    pub message_type: Option<String>,
    pub errors: Vec<String>,
}
```

## boruna_framework::runtime

```rust
pub struct AppMessage {
    pub tag: String,
    pub payload: Value,
}

impl AppMessage {
    pub fn new(tag: impl Into<String>, payload: Value) -> Self;
    pub fn to_value(&self) -> Value;
}

pub struct CycleRecord {
    pub cycle: u64,
    pub message: AppMessage,
    pub state_before: Value,
    pub state_after: Value,
    pub effects: Vec<Effect>,
    pub ui_tree: Option<Value>,
}

pub struct AppRuntime { /* private fields */ }

impl AppRuntime {
    pub fn new(module: Module) -> Result<Self, FrameworkError>;
    pub fn state(&self) -> &Value;
    pub fn cycle(&self) -> u64;
    pub fn cycle_log(&self) -> &[CycleRecord];
    pub fn policy(&self) -> &PolicySet;
    pub fn state_machine(&self) -> &StateMachine;
    pub fn send(&mut self, msg: AppMessage) -> Result<(Value, Vec<Effect>, Option<Value>), FrameworkError>;
    pub fn view(&self) -> Result<Value, FrameworkError>;
    pub fn snapshot(&self) -> String;
    pub fn rewind(&mut self, cycle: u64) -> Result<(), FrameworkError>;
    pub fn diff_from(&self, cycle: u64) -> Vec<StateDiff>;
}
```

## boruna_framework::effect

```rust
pub struct Effect {
    pub kind: EffectKind,
    pub payload: Value,
    pub callback_tag: String,
}

pub enum EffectKind {
    HttpRequest, DbQuery, FsRead, FsWrite,
    Timer, Random, SpawnActor, EmitUi,
}

impl EffectKind {
    pub fn from_str(s: &str) -> Option<Self>;
    pub fn capability_name(&self) -> &'static str;
    pub fn as_str(&self) -> &'static str;
}

pub fn parse_effects(effects_value: &Value) -> Vec<Effect>;
pub fn parse_update_result(value: &Value) -> Option<(Value, Vec<Effect>)>;
```

## boruna_framework::state

```rust
pub struct StateSnapshot {
    pub cycle: u64,
    pub state: Value,
    pub json: String,
}

pub struct StateDiff {
    pub field_index: usize,
    pub field_name: String,
    pub old_value: Value,
    pub new_value: Value,
}

pub struct StateMachine { /* private fields */ }

impl StateMachine {
    pub fn new(initial_state: Value) -> Self;
    pub fn current(&self) -> &Value;
    pub fn cycle(&self) -> u64;
    pub fn history(&self) -> &[StateSnapshot];
    pub fn transition(&mut self, new_state: Value);
    pub fn snapshot(&self) -> String;
    pub fn restore(&mut self, json: &str) -> Result<(), FrameworkError>;
    pub fn diff_from_cycle(&self, cycle: u64) -> Vec<StateDiff>;
    pub fn diff_values(old: &Value, new: &Value) -> Vec<StateDiff>;
    pub fn rewind(&mut self, target_cycle: u64) -> Result<(), FrameworkError>;
}
```

## boruna_framework::ui

```rust
pub struct UINode {
    pub tag: String,
    pub props: Vec<(String, Value)>,
    pub children: Vec<UINode>,
}

impl UINode {
    pub fn new(tag: impl Into<String>) -> Self;
    pub fn with_prop(self, key: impl Into<String>, value: Value) -> Self;
    pub fn with_child(self, child: UINode) -> Self;
}

pub fn value_to_ui_tree(value: &Value) -> UINode;
pub fn ui_tree_to_value(node: &UINode) -> Value;
```

## boruna_framework::policy

```rust
pub struct PolicySet {
    pub capabilities: Vec<String>,
    pub max_effects_per_cycle: u64,
    pub max_steps: u64,
}

impl PolicySet {
    pub fn allow_all() -> Self;
    pub fn from_value(value: &Value) -> Self;
    pub fn check_effect(&self, effect: &Effect) -> Result<(), FrameworkError>;
    pub fn check_batch(&self, effects: &[Effect]) -> Result<(), FrameworkError>;
}
```

## boruna_framework::testing

```rust
pub struct TestHarness { /* private fields */ }

impl TestHarness {
    pub fn from_source(source: &str) -> Result<Self, FrameworkError>;
    pub fn state(&self) -> &Value;
    pub fn cycle(&self) -> u64;
    pub fn send(&mut self, msg: AppMessage) -> Result<(Value, Vec<Effect>), FrameworkError>;
    pub fn simulate(&mut self, messages: Vec<AppMessage>) -> Result<Value, FrameworkError>;
    pub fn assert_state_field(field_index: usize, expected: &Value) -> Result<(), FrameworkError>;
    pub fn assert_effects(expected_kinds: &[&str]) -> Result<(), FrameworkError>;
    pub fn assert_state(expected: &Value) -> Result<(), FrameworkError>;
    pub fn cycle_log(&self) -> &[CycleRecord];
    pub fn snapshot(&self) -> String;
    pub fn rewind(&mut self, cycle: u64) -> Result<(), FrameworkError>;
    pub fn replay_verify(&self, source: &str, messages: Vec<AppMessage>) -> Result<bool, FrameworkError>;
    pub fn view(&self) -> Result<Value, FrameworkError>;
    pub fn runtime(&self) -> &AppRuntime;
}

pub fn simulate_messages(source: &str, messages: Vec<AppMessage>) -> Result<Value, FrameworkError>;
```
