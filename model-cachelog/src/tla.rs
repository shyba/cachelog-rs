#![cfg(feature = "tla-connect")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use itf::Value;
use itf::value::Record;
use serde::Serialize;
use tla_connect::{
    BuilderError, Driver, DriverError, ExtractState, State, StateEmitter, Step,
    TraceValidatorConfig,
};

use crate::{
    CacheId, ComparableCleanRecord, ComparableDirtyRecord, ComparableDurableValue, ComparableState,
    ComparableVisibleRef, ModelConfig, ModelState, ModelStep, VisibleRef, WriteId,
};

pub struct ModelDriver {
    pub config: ModelConfig,
    pub state: ModelState,
}

impl ModelDriver {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            state: ModelState::new(config),
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
    type State = ComparableState;

    fn step(&mut self, step: &Step) -> Result<(), DriverError> {
        let action = step.action_taken.clone();
        let model_step = match action.as_str() {
            "WriterWrite" => {
                let (key, value) = parse_key_value(&step.nondet_picks)?;
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

impl State for ComparableState {
    fn from_spec(value: &Value) -> Result<Self, DriverError> {
        let record = expect_record(value)?;

        Ok(ComparableState {
            visible: parse_visible(record.get("visible").ok_or_else(missing("visible"))?)?,
            write_store: parse_dirty_map(
                record.get("writeStore").ok_or_else(missing("writeStore"))?,
            )?,
            write_hist: parse_dirty_map(record.get("writeHist").ok_or_else(missing("writeHist"))?)?,
            dirty_q: parse_id_sequence(record.get("dirtyQ").ok_or_else(missing("dirtyQ"))?)?,
            cache_store: parse_clean_map(
                record.get("cacheStore").ok_or_else(missing("cacheStore"))?,
            )?,
            durable: parse_durable_map(record.get("durable").ok_or_else(missing("durable"))?)?,
            flushed: parse_id_sequence(record.get("flushed").ok_or_else(missing("flushed"))?)?,
            created_dirty: parse_id_set(
                record
                    .get("createdDirty")
                    .ok_or_else(missing("createdDirty"))?,
            )?,
            next_write: parse_number(record.get("nextWrite").ok_or_else(missing("nextWrite"))?)?,
            next_cache: parse_number(record.get("nextCache").ok_or_else(missing("nextCache"))?)?,
            crashed: parse_bool(record.get("crashed").ok_or_else(missing("crashed"))?)?,
            bad_read: parse_bool(record.get("badRead").ok_or_else(missing("badRead"))?)?,
        })
    }
}

impl ExtractState<ModelDriver> for ComparableState {
    fn from_driver(driver: &ModelDriver) -> Result<Self, DriverError> {
        Ok(ComparableState::from(&driver.state))
    }
}

fn parse_key_value(value: &Value) -> Result<(usize, usize), DriverError> {
    let record = expect_record(value)?;
    Ok((
        parse_number(record.get("key").ok_or_else(missing("key"))?)?,
        parse_number(record.get("value").ok_or_else(missing("value"))?)?,
    ))
}

fn parse_id(value: &Value, field: &str) -> Result<usize, DriverError> {
    let record = expect_record(value)?;
    parse_number(record.get(field).ok_or_else(missing(field))?)
}

fn parse_visible(value: &Value) -> Result<BTreeMap<usize, ComparableVisibleRef>, DriverError> {
    let mut map = BTreeMap::new();
    for (key, value) in parse_function_map(value)? {
        let key = parse_number(&key)?;
        if let Some(visible) = parse_visible_ref(&value)? {
            map.insert(key, visible);
        }
    }
    Ok(map)
}

fn parse_visible_ref(value: &Value) -> Result<Option<ComparableVisibleRef>, DriverError> {
    let record = expect_record(value)?;
    let kind = parse_string(record.get("kind").ok_or_else(missing("kind"))?)?;
    match kind.as_str() {
        "None" => Ok(None),
        "Dirty" => Ok(Some(ComparableVisibleRef::Dirty(parse_number(
            record.get("id").ok_or_else(missing("id"))?,
        )?))),
        "Clean" => Ok(Some(ComparableVisibleRef::Clean(parse_number(
            record.get("id").ok_or_else(missing("id"))?,
        )?))),
        other => Err(DriverError::StateExtraction(format!(
            "unknown visible kind {other}"
        ))),
    }
}

fn parse_dirty_map(value: &Value) -> Result<BTreeMap<WriteId, ComparableDirtyRecord>, DriverError> {
    let mut map = BTreeMap::new();
    for (key, value) in parse_function_map(value)? {
        let id = parse_number(&key)?;
        if let Some(record) = parse_dirty_record(&value)? {
            map.insert(id, record);
        }
    }
    Ok(map)
}

fn parse_clean_map(value: &Value) -> Result<BTreeMap<CacheId, ComparableCleanRecord>, DriverError> {
    let mut map = BTreeMap::new();
    for (key, value) in parse_function_map(value)? {
        let id = parse_number(&key)?;
        if let Some(record) = parse_clean_record(&value)? {
            map.insert(id, record);
        }
    }
    Ok(map)
}

fn parse_durable_map(
    value: &Value,
) -> Result<BTreeMap<usize, ComparableDurableValue>, DriverError> {
    let mut map = BTreeMap::new();
    for (key, value) in parse_function_map(value)? {
        let key_id = parse_number(&key)?;
        if let Some(record) = parse_durable_record(key_id, &value)? {
            map.insert(key_id, record);
        }
    }
    Ok(map)
}

fn parse_dirty_record(value: &Value) -> Result<Option<ComparableDirtyRecord>, DriverError> {
    let record = expect_record(value)?;
    if !parse_bool(record.get("present").ok_or_else(missing("present"))?)? {
        return Ok(None);
    }
    Ok(Some(ComparableDirtyRecord {
        id: parse_number(record.get("id").ok_or_else(missing("id"))?)?,
        key: parse_number(record.get("key").ok_or_else(missing("key"))?)?,
        value: parse_number(record.get("value").ok_or_else(missing("value"))?)?,
    }))
}

fn parse_clean_record(value: &Value) -> Result<Option<ComparableCleanRecord>, DriverError> {
    let record = expect_record(value)?;
    if !parse_bool(record.get("present").ok_or_else(missing("present"))?)? {
        return Ok(None);
    }
    Ok(Some(ComparableCleanRecord {
        id: parse_number(record.get("id").ok_or_else(missing("id"))?)?,
        key: parse_number(record.get("key").ok_or_else(missing("key"))?)?,
        value: parse_number(record.get("value").ok_or_else(missing("value"))?)?,
    }))
}

fn parse_durable_record(
    key: usize,
    value: &Value,
) -> Result<Option<ComparableDurableValue>, DriverError> {
    let record = expect_record(value)?;
    if !parse_bool(record.get("present").ok_or_else(missing("present"))?)? {
        return Ok(None);
    }
    Ok(Some(ComparableDurableValue {
        key,
        value: parse_number(record.get("value").ok_or_else(missing("value"))?)?,
        seq: parse_number(record.get("seq").ok_or_else(missing("seq"))?)?,
    }))
}

fn parse_id_sequence(value: &Value) -> Result<Vec<usize>, DriverError> {
    match value {
        Value::List(items) => items.iter().map(parse_number).collect(),
        Value::Tuple(items) => items.iter().map(parse_number).collect(),
        _ => Err(DriverError::StateExtraction(format!(
            "expected sequence, got {value:?}"
        ))),
    }
}

fn parse_id_set(value: &Value) -> Result<BTreeSet<usize>, DriverError> {
    match value {
        Value::Set(items) => items.iter().map(parse_number).collect(),
        Value::List(items) => items.iter().map(parse_number).collect(),
        _ => Err(DriverError::StateExtraction(format!(
            "expected set, got {value:?}"
        ))),
    }
}

fn parse_function_map(value: &Value) -> Result<Vec<(Value, Value)>, DriverError> {
    match value {
        Value::Map(map) => Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        Value::Record(rec) => rec
            .iter()
            .map(|(k, v)| Ok((Value::String(k.clone()), v.clone())))
            .collect(),
        _ => Err(DriverError::StateExtraction(format!(
            "expected function map, got {value:?}"
        ))),
    }
}

fn expect_record(value: &Value) -> Result<&Record, DriverError> {
    match value {
        Value::Record(record) => Ok(record),
        _ => Err(DriverError::StateExtraction(format!(
            "expected record, got {value:?}"
        ))),
    }
}

fn parse_number(value: &Value) -> Result<usize, DriverError> {
    match value {
        Value::Number(n) if *n >= 0 => Ok(*n as usize),
        Value::BigInt(n) => n
            .to_string()
            .parse::<usize>()
            .map_err(|e| DriverError::StateExtraction(e.to_string())),
        Value::String(s) => s
            .parse::<usize>()
            .map_err(|e| DriverError::StateExtraction(e.to_string())),
        _ => Err(DriverError::StateExtraction(format!(
            "expected non-negative number, got {value:?}"
        ))),
    }
}

fn parse_bool(value: &Value) -> Result<bool, DriverError> {
    match value {
        Value::Bool(v) => Ok(*v),
        _ => Err(DriverError::StateExtraction(format!(
            "expected bool, got {value:?}"
        ))),
    }
}

fn parse_string(value: &Value) -> Result<String, DriverError> {
    match value {
        Value::String(v) => Ok(v.clone()),
        _ => Err(DriverError::StateExtraction(format!(
            "expected string, got {value:?}"
        ))),
    }
}

fn missing(field: &str) -> impl FnOnce() -> DriverError + '_ {
    move || DriverError::StateExtraction(format!("missing field {field}"))
}

#[derive(Serialize)]
struct TraceState {
    visible: Vec<TraceVisibleRef>,
    write_store: Vec<TraceDirtyRecord>,
    write_hist: Vec<TraceDirtyRecord>,
    dirty_q: Vec<WriteId>,
    cache_store: Vec<TraceCleanRecord>,
    durable: Vec<TraceDurableValue>,
    flushed: Vec<WriteId>,
    created_dirty: Vec<bool>,
    next_write: usize,
    next_cache: usize,
    crashed: bool,
    bad_read: bool,
}

#[derive(Serialize)]
struct TraceVisibleRef {
    kind: &'static str,
    id: i64,
}

#[derive(Serialize)]
struct TraceDirtyRecord {
    present: bool,
    id: i64,
    key: usize,
    value: usize,
}

#[derive(Serialize)]
struct TraceCleanRecord {
    present: bool,
    id: i64,
    key: usize,
    value: usize,
}

#[derive(Serialize)]
struct TraceDurableValue {
    present: bool,
    value: usize,
    seq: i64,
}

impl From<&ModelState> for TraceState {
    fn from(state: &ModelState) -> Self {
        let visible = state
            .visible
            .iter()
            .map(|entry| match *entry {
                VisibleRef::None => TraceVisibleRef {
                    kind: "None",
                    id: -1,
                },
                VisibleRef::Dirty(id) => TraceVisibleRef {
                    kind: "Dirty",
                    id: id as i64,
                },
                VisibleRef::Clean(id) => TraceVisibleRef {
                    kind: "Clean",
                    id: id as i64,
                },
            })
            .collect();

        let write_store = state
            .write_store
            .iter()
            .map(|record| TraceDirtyRecord {
                present: record.present,
                id: record.id.map(|id| id as i64).unwrap_or(-1),
                key: record.key,
                value: record.value,
            })
            .collect();

        let write_hist = state
            .write_hist
            .iter()
            .map(|record| TraceDirtyRecord {
                present: record.present,
                id: record.id.map(|id| id as i64).unwrap_or(-1),
                key: record.key,
                value: record.value,
            })
            .collect();

        let cache_store = state
            .cache_store
            .iter()
            .map(|record| TraceCleanRecord {
                present: record.present,
                id: record.id.map(|id| id as i64).unwrap_or(-1),
                key: record.key,
                value: record.value,
            })
            .collect();

        let durable = state
            .durable
            .iter()
            .map(|value| TraceDurableValue {
                present: value.present,
                value: value.value,
                seq: value.seq.map(|seq| seq as i64).unwrap_or(-1),
            })
            .collect();

        Self {
            visible,
            write_store,
            write_hist,
            dirty_q: state.dirty_q.clone(),
            cache_store,
            durable,
            flushed: state.flushed.clone(),
            created_dirty: state.created_dirty.clone(),
            next_write: state.next_write,
            next_cache: state.next_cache,
            crashed: state.crashed,
            bad_read: state.bad_read,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;
    use tempfile::NamedTempFile;
    use tla_connect::{TraceResult, replay_trace_str, validate_trace};

    use super::*;

    #[test]
    fn replay_inline_trace() {
        let trace = json!({
            "#meta": {},
            "vars": [
                "visible","writeStore","writeHist","dirtyQ","cacheStore","durable",
                "flushed","createdDirty","nextWrite","nextCache","crashed","badRead"
            ],
            "states": [
                {
                    "#meta": {"index": 0},
                    "action_taken": "WriterWrite",
                    "nondet_picks": {"key": 0, "value": 1},
                    "visible": {"#map": [[0, {"kind":"Dirty","id":0}], [1, {"kind":"None","id":-1}]]},
                    "writeStore": {"#map": [[0, {"present": true, "id": 0, "key": 0, "value": 1}], [1, {"present": false, "id": -1, "key": 0, "value": 0}], [2, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "writeHist": {"#map": [[0, {"present": true, "id": 0, "key": 0, "value": 1}], [1, {"present": false, "id": -1, "key": 0, "value": 0}], [2, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "dirtyQ": [0],
                    "cacheStore": {"#map": [[0, {"present": false, "id": -1, "key": 0, "value": 0}], [1, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "durable": {"#map": [[0, {"present": false, "value": 0, "seq": -1}], [1, {"present": false, "value": 0, "seq": -1}]]},
                    "flushed": [],
                    "createdDirty": {"#set": [0]},
                    "nextWrite": 1,
                    "nextCache": 0,
                    "crashed": false,
                    "badRead": false
                },
                {
                    "#meta": {"index": 1},
                    "action_taken": "FlusherFlushNext",
                    "nondet_picks": {"kind":"None","id":-1},
                    "visible": {"#map": [[0, {"kind":"Dirty","id":0}], [1, {"kind":"None","id":-1}]]},
                    "writeStore": {"#map": [[0, {"present": true, "id": 0, "key": 0, "value": 1}], [1, {"present": false, "id": -1, "key": 0, "value": 0}], [2, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "writeHist": {"#map": [[0, {"present": true, "id": 0, "key": 0, "value": 1}], [1, {"present": false, "id": -1, "key": 0, "value": 0}], [2, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "dirtyQ": [],
                    "cacheStore": {"#map": [[0, {"present": false, "id": -1, "key": 0, "value": 0}], [1, {"present": false, "id": -1, "key": 0, "value": 0}]]},
                    "durable": {"#map": [[0, {"present": true, "value": 1, "seq": 0}], [1, {"present": false, "value": 0, "seq": -1}]]},
                    "flushed": [0],
                    "createdDirty": {"#set": [0]},
                    "nextWrite": 1,
                    "nextCache": 0,
                    "crashed": false,
                    "badRead": false
                }
            ]
        });

        let _ = replay_trace_str(
            || ModelDriver::new(ModelConfig::tla_small()),
            &trace.to_string(),
        )
        .unwrap();
    }

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

    #[test]
    fn trace_validation_with_fake_apalache() {
        let trace_file = NamedTempFile::new().unwrap();
        let mut emitter = StateEmitter::new(trace_file.path()).unwrap();

        let mut driver = ModelDriver::new(ModelConfig::tla_small());
        driver.emit_state(&mut emitter, "init").unwrap();
        driver
            .state
            .apply_step(driver.config, ModelStep::WriterWrite { key: 0, value: 1 })
            .unwrap();
        driver.emit_state(&mut emitter, "WriterWrite").unwrap();
        driver
            .state
            .apply_step(driver.config, ModelStep::FlusherFlushNext)
            .unwrap();
        driver.emit_state(&mut emitter, "FlusherFlushNext").unwrap();
        emitter.finish().unwrap();

        let spec = manifest_path("formal/CacheLogVisibleRefsTrace.tla");
        let fake_apalache = fake_apalache_script();
        let cfg = tla_connect::TraceValidatorConfig::builder()
            .trace_spec(spec)
            .apalache_bin(fake_apalache.to_string_lossy().to_string())
            .build()
            .unwrap();
        let result = validate_trace(&cfg, trace_file.path()).unwrap();
        assert!(matches!(result, TraceResult::Valid));
    }

    #[test]
    #[ignore = "requires a real apalache-mc installation"]
    fn trace_validation_roundtrip() {
        let trace_file = NamedTempFile::new().unwrap();
        let mut emitter = StateEmitter::new(trace_file.path()).unwrap();

        let mut driver = ModelDriver::new(ModelConfig::tla_small());
        driver.emit_state(&mut emitter, "init").unwrap();
        driver
            .state
            .apply_step(driver.config, ModelStep::WriterWrite { key: 0, value: 1 })
            .unwrap();
        driver.emit_state(&mut emitter, "WriterWrite").unwrap();
        driver
            .state
            .apply_step(driver.config, ModelStep::FlusherFlushNext)
            .unwrap();
        driver.emit_state(&mut emitter, "FlusherFlushNext").unwrap();
        emitter.finish().unwrap();

        let spec = manifest_path("formal/CacheLogVisibleRefsTrace.tla");
        let cfg = ModelDriver::trace_validator_config(spec.as_path()).unwrap();
        let result = validate_trace(&cfg, trace_file.path()).unwrap();
        assert!(matches!(result, TraceResult::Valid));
    }

    fn manifest_path(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    fn fake_apalache_script() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.keep();
        let path = dir_path.join("fake-apalache.sh");
        let script = r#"#!/usr/bin/env bash
set -euo pipefail

last="${@: -1}"
spec_dir="$(dirname "$last")"

test -f "$last"
test -f "$spec_dir/TraceData.tla"
grep -q "TraceLog ==" "$spec_dir/TraceData.tla"
grep -q "MODULE CacheLogVisibleRefsTrace" "$last"

exit 12
"#;
        fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }
}
