use std::env;
use std::fs;

use cachelog_model::{ModelConfig, ModelState, ModelStep};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("emit-initial-state") => {
            let state = ModelState::new(ModelConfig::tla_small());
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Some("apply-step") => {
            let state_path = args.next().ok_or("missing state path")?;
            let step_path = args.next().ok_or("missing step path")?;
            let mut state: ModelState = serde_json::from_str(&fs::read_to_string(state_path)?)?;
            let step: ModelStep = serde_json::from_str(&fs::read_to_string(step_path)?)?;
            state.apply_step(ModelConfig::tla_small(), step)?;
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Some("replay-trace") => {
            let trace_path = args.next().ok_or("missing trace path")?;
            let mut state = ModelState::new(ModelConfig::tla_small());
            let steps: Vec<ModelStep> = serde_json::from_str(&fs::read_to_string(trace_path)?)?;
            for step in steps {
                state.apply_step(ModelConfig::tla_small(), step)?;
            }
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Some("check-state") => {
            let state_path = args.next().ok_or("missing state path")?;
            let state: ModelState = serde_json::from_str(&fs::read_to_string(state_path)?)?;
            let report = state.check_invariants(ModelConfig::tla_small());
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.is_ok() {
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  emit-initial-state");
            eprintln!("  apply-step <state.json> <step.json>");
            eprintln!("  replay-trace <trace.json>");
            eprintln!("  check-state <state.json>");
            std::process::exit(2);
        }
    }
    Ok(())
}
