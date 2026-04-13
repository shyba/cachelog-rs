#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
formal_dir="${repo_root}/model-cachelog/formal"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

cp "${formal_dir}/CacheLogVisibleRefsTrace.tla" "$tmpdir/"

cat > "$tmpdir/TraceData.tla" <<'EOF'
---- MODULE TraceData ----
EXTENDS Integers, Sequences

TraceLog == <<
  [ action |-> "init",
    visible |-> << [kind |-> "None", id |-> -1], [kind |-> "None", id |-> -1] >>,
    write_store |-> <<
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    write_hist |-> <<
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    dirty_q |-> <<>>,
    cache_store |-> <<
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    durable |-> <<
      [present |-> FALSE, value |-> 0, seq |-> -1],
      [present |-> FALSE, value |-> 0, seq |-> -1]
    >>,
    flushed |-> <<>>,
    created_dirty |-> << FALSE, FALSE, FALSE >>,
    next_write |-> 0,
    next_cache |-> 0,
    crashed |-> FALSE,
    bad_read |-> FALSE
  ],
  [ action |-> "WriterWrite",
    visible |-> << [kind |-> "Dirty", id |-> 0], [kind |-> "None", id |-> -1] >>,
    write_store |-> <<
      [present |-> TRUE, id |-> 0, key |-> 0, value |-> 1],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    write_hist |-> <<
      [present |-> TRUE, id |-> 0, key |-> 0, value |-> 1],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    dirty_q |-> <<0>>,
    cache_store |-> <<
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0],
      [present |-> FALSE, id |-> -1, key |-> 0, value |-> 0]
    >>,
    durable |-> <<
      [present |-> FALSE, value |-> 0, seq |-> -1],
      [present |-> FALSE, value |-> 0, seq |-> -1]
    >>,
    flushed |-> <<>>,
    created_dirty |-> << TRUE, FALSE, FALSE >>,
    next_write |-> 1,
    next_cache |-> 0,
    crashed |-> FALSE,
    bad_read |-> FALSE
  ]
>>

TraceActions == <<"init", "WriterWrite">>

====
EOF

cat > "$tmpdir/MC.cfg" <<'EOF'
SPECIFICATION Spec
EOF

(
  cd "$tmpdir"
  tlc -deadlock -config MC.cfg CacheLogVisibleRefsTrace.tla
)
