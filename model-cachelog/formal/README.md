# Trace Validation Spec

This directory contains the TLA+ TraceSpec used by `tla-connect` Approach 3.

- [CacheLogVisibleRefsTrace.tla](/home/user/repos/tableflip-rs/model-cachelog/formal/CacheLogVisibleRefsTrace.tla)

Notes:
- `validate_trace()` writes a generated `TraceData.tla` into a temporary copy of this
  directory at runtime.
- The spec is intentionally based on the full model state emitted by
  [`ModelDriver::emit_state`](/home/user/repos/tableflip-rs/model-cachelog/src/tla.rs), not
  the sparse comparable replay state. That keeps the TLA side aligned with the Rust model and
  avoids awkward JSON-object map encoding.
- The current spec is fixed to the small model configuration used by
  [`ModelConfig::tla_small()`](/home/user/repos/tableflip-rs/model-cachelog/src/model.rs).
