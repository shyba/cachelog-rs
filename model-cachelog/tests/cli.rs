use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cachelog_model::{DurableValue, ModelConfig, ModelState, VisibleRef};
use tempfile::NamedTempFile;

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn model_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cachelog-model")
}

#[test]
fn emit_initial_state_outputs_valid_state() {
    let output = Command::new(model_bin())
        .arg("emit-initial-state")
        .output()
        .unwrap();

    assert!(output.status.success());
    let state: ModelState = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(state, ModelState::new(ModelConfig::tla_small()));
}

#[test]
fn apply_step_fixture_updates_state() {
    let state_file = NamedTempFile::new().unwrap();
    fs::write(
        state_file.path(),
        serde_json::to_vec_pretty(&ModelState::new(ModelConfig::tla_small())).unwrap(),
    )
    .unwrap();

    let output = Command::new(model_bin())
        .arg("apply-step")
        .arg(state_file.path())
        .arg(fixture_path("step_writer_write_k0_v1.json"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let state: ModelState = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(state.visible[0], VisibleRef::Dirty(0));
    assert_eq!(state.dirty_q, vec![0]);
}

#[test]
fn replay_trace_fixture_reaches_expected_state() {
    let output = Command::new(model_bin())
        .arg("replay-trace")
        .arg(fixture_path("trace_write_flush_drop.json"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let state: ModelState = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(state.visible[0], VisibleRef::None);
    assert_eq!(state.durable[0], DurableValue::present(1, 0));
}

#[test]
fn check_state_succeeds_for_valid_state() {
    let output = Command::new(model_bin())
        .arg("replay-trace")
        .arg(fixture_path("trace_write_flush_drop.json"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let state_file = NamedTempFile::new().unwrap();
    fs::write(state_file.path(), &output.stdout).unwrap();

    let check = Command::new(model_bin())
        .arg("check-state")
        .arg(state_file.path())
        .output()
        .unwrap();

    assert!(check.status.success());
    let report: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(report["failures"], serde_json::json!([]));
}

#[test]
fn check_state_fails_for_invalid_state() {
    let mut state = ModelState::new(ModelConfig::tla_small());
    state.visible[0] = VisibleRef::Dirty(0);

    let state_file = NamedTempFile::new().unwrap();
    fs::write(
        state_file.path(),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();

    let check = Command::new(model_bin())
        .arg("check-state")
        .arg(state_file.path())
        .output()
        .unwrap();

    assert!(!check.status.success());
    let report: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "DirtyRefsLive")
    );
}
