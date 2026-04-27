#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use std::sync::OnceLock;
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use std::sync::atomic::AtomicU64 as StdAtomicU64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(feature = "dev-tools")]
pub struct CoalescedSingleWritePerfCounters {
    pub write_id_ns_total: u64,
    pub remove_clean_ns_total: u64,
    pub snapshot_check_ns_total: u64,
    pub active_publish_ns_total: u64,
    pub snapshot_inflight_check_ns_total: u64,
    pub snapshot_draining_check_ns_total: u64,
    pub snapshot_inflight_lock_ns_total: u64,
    pub snapshot_inflight_scan_ns_total: u64,
    pub snapshot_inflight_index_lookup_ns_total: u64,
    pub snapshot_inflight_record_get_ns_total: u64,
    pub snapshot_draining_lock_ns_total: u64,
    pub snapshot_draining_read_ns_total: u64,
    pub active_map_load_ns_total: u64,
    pub active_entry_publish_ns_total: u64,
    pub active_entry_lookup_ns_total: u64,
    pub active_entry_occupied_ns_total: u64,
    pub active_entry_vacant_ns_total: u64,
    pub active_entry_vacant_box_ns_total: u64,
    pub active_entry_vacant_insert_ns_total: u64,
    pub dirty_count_add_ns_total: u64,
}

#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_WRITE_ID_NS_TOTAL: StdAtomicU64 = StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_REMOVE_CLEAN_NS_TOTAL: StdAtomicU64 = StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_CHECK_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_PUBLISH_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_CHECK_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_CHECK_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_LOCK_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_SCAN_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_INDEX_LOOKUP_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_RECORD_GET_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_LOCK_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_READ_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_MAP_LOAD_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_PUBLISH_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_LOOKUP_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_OCCUPIED_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_BOX_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_INSERT_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) static PERF_COALESCED_SINGLE_DIRTY_COUNT_ADD_NS_TOTAL: StdAtomicU64 =
    StdAtomicU64::new(0);

#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
pub(crate) fn coalesced_perf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("CACHELOG_PERF_COUNTERS")
            .ok()
            .map(|raw| raw != "0")
            .unwrap_or(false)
    })
}
