use boruna_bytecode::Value;
use serde_json;

use crate::error::FrameworkError;

/// State snapshot for time-travel debugging.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub cycle: u64,
    pub state: Value,
    pub json: String,
}

/// Diff entry between two states.
#[derive(Debug, Clone)]
pub struct StateDiff {
    pub field_index: usize,
    pub field_name: String,
    pub old_value: Value,
    pub new_value: Value,
}

/// State machine that manages the application state lifecycle.
pub struct StateMachine {
    current: Value,
    history: Vec<StateSnapshot>,
    cycle: u64,
    max_history: usize,
}

impl StateMachine {
    pub fn new(initial_state: Value) -> Self {
        let json = serde_json::to_string(&initial_state).unwrap_or_default();
        let snapshot = StateSnapshot {
            cycle: 0,
            state: initial_state.clone(),
            json,
        };
        StateMachine {
            current: initial_state,
            history: vec![snapshot],
            cycle: 0,
            max_history: 1000,
        }
    }

    pub fn current(&self) -> &Value {
        &self.current
    }

    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    pub fn history(&self) -> &[StateSnapshot] {
        &self.history
    }

    /// Transition to a new state. Records the snapshot.
    pub fn transition(&mut self, new_state: Value) {
        self.cycle += 1;
        let json = serde_json::to_string(&new_state).unwrap_or_default();
        let snapshot = StateSnapshot {
            cycle: self.cycle,
            state: new_state.clone(),
            json,
        };
        self.history.push(snapshot);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
        self.current = new_state;
    }

    /// Serialize the current state to JSON.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&self.current).unwrap_or_default()
    }

    /// Restore state from JSON.
    pub fn restore(&mut self, json: &str) -> Result<(), FrameworkError> {
        let state: Value = serde_json::from_str(json)
            .map_err(|e| FrameworkError::State(format!("invalid state JSON: {e}")))?;
        self.transition(state);
        Ok(())
    }

    /// Compute the diff between the current state and a previous cycle.
    pub fn diff_from_cycle(&self, cycle: u64) -> Vec<StateDiff> {
        let old = self.history.iter()
            .find(|s| s.cycle == cycle)
            .map(|s| &s.state);

        match old {
            Some(old_state) => Self::diff_values(old_state, &self.current),
            None => Vec::new(),
        }
    }

    /// Diff two record values field by field.
    pub fn diff_values(old: &Value, new: &Value) -> Vec<StateDiff> {
        let mut diffs = Vec::new();

        match (old, new) {
            (Value::Record { fields: old_fields, .. },
             Value::Record { fields: new_fields, .. }) => {
                let max = old_fields.len().max(new_fields.len());
                for i in 0..max {
                    let old_val = old_fields.get(i).cloned().unwrap_or(Value::Unit);
                    let new_val = new_fields.get(i).cloned().unwrap_or(Value::Unit);
                    if old_val != new_val {
                        diffs.push(StateDiff {
                            field_index: i,
                            field_name: format!("field_{i}"),
                            old_value: old_val,
                            new_value: new_val,
                        });
                    }
                }
            }
            _ => {
                if old != new {
                    diffs.push(StateDiff {
                        field_index: 0,
                        field_name: "root".into(),
                        old_value: old.clone(),
                        new_value: new.clone(),
                    });
                }
            }
        }

        diffs
    }

    /// Time-travel: rewind to a specific cycle.
    pub fn rewind(&mut self, target_cycle: u64) -> Result<(), FrameworkError> {
        let snapshot = self.history.iter()
            .find(|s| s.cycle == target_cycle)
            .ok_or_else(|| FrameworkError::State(
                format!("cycle {target_cycle} not in history")
            ))?;
        self.current = snapshot.state.clone();
        self.cycle = snapshot.cycle;
        Ok(())
    }
}
