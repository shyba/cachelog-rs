use std::path::Path;

use tla_connect::{BuilderError, Driver, DriverError, StateEmitter, Step, TraceValidatorConfig};

use crate::{ModelConfig, ModelStep};

use super::trace::TraceState;
use super::value::parse_id;

pub struct ModelDriver {
    pub config: ModelConfig,
    pub state: crate::ModelState,
}

impl ModelDriver {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            state: crate::ModelState::new(config),
        }
    }

    pub fn emit_state(&self, emitter: &mut StateEmitter, action: &str) -> Result<(), DriverError> {
        let state = TraceState::from(&self.state);
        emitter
            .emit(action, &state)
            .map_err(|e| DriverError::StateExtraction(e.to_string()))
    }

    pub fn trace_validator_config(trace_spec: &Path) -> Result<TraceValidatorConfig, BuilderError> {
        TraceValidatorConfig::builder()
            .trace_spec(trace_spec)
            .build()
    }
}

impl Driver for ModelDriver {
    type State = crate::ComparableState;

    fn step(&mut self, step: &Step) -> Result<(), DriverError> {
        let action = step.action_taken.clone();
        let model_step = match action.as_str() {
            "WriterWrite" => {
                let (key, value) = super::value::parse_key_value(&step.nondet_picks)?;
                ModelStep::WriterWrite { key, value }
            }
            "FlusherFlushNext" => ModelStep::FlusherFlushNext,
            "FlusherDrop" => ModelStep::FlusherDrop {
                id: parse_id(&step.nondet_picks, "id")?,
            },
            "CacheInsert" => ModelStep::CacheInsert {
                key: parse_id(&step.nondet_picks, "key")?,
            },
            "CacheDropRecord" => ModelStep::CacheDropRecord {
                id: parse_id(&step.nondet_picks, "id")?,
            },
            "CacheCleanupVisible" => ModelStep::CacheCleanupVisible {
                id: parse_id(&step.nondet_picks, "id")?,
            },
            "ReaderStep" => ModelStep::ReaderStep {
                key: parse_id(&step.nondet_picks, "key")?,
            },
            "Crash" => ModelStep::Crash,
            other => return Err(DriverError::UnknownAction(other.to_string())),
        };

        self.state
            .apply_step(self.config, model_step)
            .map_err(|e| DriverError::ActionFailed {
                action,
                reason: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;
    use tla_connect::StateEmitter;

    use crate::ModelConfig;

    use super::ModelDriver;

    #[test]
    fn emits_ndjson_state() {
        let file = NamedTempFile::new().unwrap();
        let mut emitter = StateEmitter::new(file.path()).unwrap();
        let driver = ModelDriver::new(ModelConfig::tla_small());
        driver.emit_state(&mut emitter, "init").unwrap();
        emitter.finish().unwrap();
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("\"action\":\"init\""));
        assert!(content.contains("\"visible\""));
    }
}
