use serde::Serialize;
use tla_connect::{DriverError, ExtractState, State};

use crate::{ComparableState, ModelState, VisibleRef, WriteId};

use super::value::{
    expect_record, missing, parse_bool, parse_clean_map, parse_dirty_map, parse_durable_map,
    parse_id_sequence, parse_id_set, parse_number, parse_visible,
};

#[derive(Serialize)]
pub(crate) struct TraceState {
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

impl State for ComparableState {
    fn from_spec(value: &itf::Value) -> Result<Self, DriverError> {
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

impl ExtractState<super::driver::ModelDriver> for ComparableState {
    fn from_driver(driver: &super::driver::ModelDriver) -> Result<Self, DriverError> {
        Ok(ComparableState::from(&driver.state))
    }
}
