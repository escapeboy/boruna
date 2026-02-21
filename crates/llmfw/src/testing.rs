use boruna_bytecode::Value;

use crate::effect::Effect;
use crate::executor::EffectExecutor;
use crate::error::FrameworkError;
use crate::runtime::{AppMessage, AppRuntime, CycleRecord};

/// Test harness for framework applications.
///
/// Provides utilities for testing apps without a host UI.
pub struct TestHarness {
    runtime: AppRuntime,
}

impl TestHarness {
    /// Create a test harness from source code.
    pub fn from_source(source: &str) -> Result<Self, FrameworkError> {
        let module = boruna_compiler::compile("test", source)?;
        let runtime = AppRuntime::new(module)?;
        Ok(TestHarness { runtime })
    }

    /// Get the current state.
    pub fn state(&self) -> &Value {
        self.runtime.state()
    }

    /// Get the current cycle.
    pub fn cycle(&self) -> u64 {
        self.runtime.cycle()
    }

    /// Send a single message.
    pub fn send(&mut self, msg: AppMessage) -> Result<(Value, Vec<Effect>), FrameworkError> {
        let (state, effects, _) = self.runtime.send(msg)?;
        Ok((state, effects))
    }

    /// Send a message and execute effects, returning callback messages.
    ///
    /// Uses the provided `EffectExecutor` to dispatch effects and produce
    /// callback `AppMessage`s that can be fed back into `send()`.
    pub fn send_with_effects(
        &mut self,
        msg: AppMessage,
        executor: &mut dyn EffectExecutor,
    ) -> Result<(Value, Vec<AppMessage>), FrameworkError> {
        let (state, callbacks, _) = self.runtime.send_with_executor(msg, executor)?;
        Ok((state, callbacks))
    }

    /// Simulate a sequence of messages. Returns the final state.
    pub fn simulate(&mut self, messages: Vec<AppMessage>) -> Result<Value, FrameworkError> {
        let mut state = self.runtime.state().clone();
        for msg in messages {
            let (new_state, _, _) = self.runtime.send(msg)?;
            state = new_state;
        }
        Ok(state)
    }

    /// Assert that a record state has a specific field value.
    pub fn assert_state_field(
        &self,
        field_index: usize,
        expected: &Value,
    ) -> Result<(), FrameworkError> {
        match self.runtime.state() {
            Value::Record { fields, .. } => {
                let actual = fields.get(field_index)
                    .ok_or_else(|| FrameworkError::State(
                        format!("field index {field_index} out of bounds")
                    ))?;
                if actual != expected {
                    Err(FrameworkError::State(format!(
                        "field[{field_index}]: expected {expected}, got {actual}"
                    )))
                } else {
                    Ok(())
                }
            }
            other => Err(FrameworkError::State(format!(
                "expected Record state, got {}", other.type_name()
            ))),
        }
    }

    /// Assert that the last cycle produced effects of the given kinds.
    pub fn assert_effects(
        &self,
        expected_kinds: &[&str],
    ) -> Result<(), FrameworkError> {
        let log = self.runtime.cycle_log();
        let last = log.last()
            .ok_or_else(|| FrameworkError::State("no cycles recorded".into()))?;

        let actual_kinds: Vec<&str> = last.effects.iter()
            .map(|e| e.kind.as_str())
            .collect();

        if actual_kinds.len() != expected_kinds.len() {
            return Err(FrameworkError::State(format!(
                "effect count mismatch: expected {:?}, got {:?}",
                expected_kinds, actual_kinds
            )));
        }

        for (i, (expected, actual)) in expected_kinds.iter().zip(actual_kinds.iter()).enumerate() {
            if expected != actual {
                return Err(FrameworkError::State(format!(
                    "effect[{i}]: expected '{}', got '{}'",
                    expected, actual
                )));
            }
        }

        Ok(())
    }

    /// Assert the current state equals an expected value.
    pub fn assert_state(&self, expected: &Value) -> Result<(), FrameworkError> {
        let actual = self.runtime.state();
        if actual != expected {
            Err(FrameworkError::State(format!(
                "state mismatch:\n  expected: {expected}\n  actual:   {actual}"
            )))
        } else {
            Ok(())
        }
    }

    /// Get the cycle log for inspection.
    pub fn cycle_log(&self) -> &[CycleRecord] {
        self.runtime.cycle_log()
    }

    /// Get the state snapshot as JSON.
    pub fn snapshot(&self) -> String {
        self.runtime.snapshot()
    }

    /// Time-travel to a previous cycle.
    pub fn rewind(&mut self, cycle: u64) -> Result<(), FrameworkError> {
        self.runtime.rewind(cycle)
    }

    /// Replay: run the same message sequence and verify identical state transitions.
    pub fn replay_verify(
        &self,
        source: &str,
        messages: Vec<AppMessage>,
    ) -> Result<bool, FrameworkError> {
        let module = boruna_compiler::compile("replay_test", source)?;
        let mut replay_runtime = AppRuntime::new(module)?;

        let original_log = self.runtime.cycle_log();

        for (i, msg) in messages.into_iter().enumerate() {
            let (state, _, _) = replay_runtime.send(msg)?;

            if let Some(original_cycle) = original_log.get(i) {
                if state != original_cycle.state_after {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Get the view tree for the current state.
    pub fn view(&self) -> Result<Value, FrameworkError> {
        self.runtime.view()
    }

    /// Get the AppRuntime for direct access.
    pub fn runtime(&self) -> &AppRuntime {
        &self.runtime
    }
}

/// Convenience: run a sequence and get final state in one call.
pub fn simulate_messages(
    source: &str,
    messages: Vec<AppMessage>,
) -> Result<Value, FrameworkError> {
    let mut harness = TestHarness::from_source(source)?;
    harness.simulate(messages)
}
