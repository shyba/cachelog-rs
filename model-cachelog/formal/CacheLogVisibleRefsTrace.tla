---- MODULE CacheLogVisibleRefsTrace ----
EXTENDS Integers, Sequences, TLC

\* validate_trace() writes TraceData.tla beside this module at runtime.
LOCAL INSTANCE TraceData

KeyCount == 2
ValueCount == 2
MaxWrite == 3
MaxCache == 2

Keys == 0..(KeyCount - 1)
Values == 0..(ValueCount - 1)
WriteIds == 0..(MaxWrite - 1)
CacheIds == 0..(MaxCache - 1)

NullToken == "null"

NoneRef == [kind |-> "None"]
DirtyRef(i) == [kind |-> "Dirty", id |-> i]
CleanRef(i) == [kind |-> "Clean", id |-> i]

NullDirty == [present |-> FALSE, id |-> NullToken, key |-> 0, value |-> 0]
NullClean == [present |-> FALSE, id |-> NullToken, key |-> 0, value |-> 0]
NullDurable == [present |-> FALSE, value |-> 0, seq |-> NullToken]

VARIABLES visible, write_store, write_hist, dirty_q, cache_store, durable,
          flushed, created_dirty, next_write, next_cache, crashed, bad_read, pos

vars == << visible, write_store, write_hist, dirty_q, cache_store, durable,
           flushed, created_dirty, next_write, next_cache, crashed, bad_read, pos >>

KeyIx(k) == k + 1
WriteIx(i) == i + 1
CacheIx(i) == i + 1

SetAt(seq, i, v) == [seq EXCEPT ![i] = v]

VisibleAt(vis, k) == vis[KeyIx(k)]
DirtyAt(store, i) == store[WriteIx(i)]
CleanAt(store, i) == store[CacheIx(i)]
DurableAt(store, k) == store[KeyIx(k)]
CreatedDirtyAt(bits, i) == bits[WriteIx(i)]

DirtyLive(store, i) == DirtyAt(store, i).present
CleanLive(store, i) == CleanAt(store, i).present

SeqSet(s) == {s[i] : i \in 1..Len(s)}

RemoveVisibleDirty(vis, id) ==
    [i \in 1..Len(vis) |-> IF vis[i] = DirtyRef(id) THEN NoneRef ELSE vis[i]]

RemoveVisibleClean(vis, id) ==
    [i \in 1..Len(vis) |-> IF vis[i] = CleanRef(id) THEN NoneRef ELSE vis[i]]

NoOp ==
    UNCHANGED << visible, write_store, write_hist, dirty_q, cache_store, durable,
                flushed, created_dirty, next_write, next_cache, crashed, bad_read >>

StateMatches(rec) ==
    /\ visible = rec.visible
    /\ write_store = rec.write_store
    /\ write_hist = rec.write_hist
    /\ dirty_q = rec.dirty_q
    /\ cache_store = rec.cache_store
    /\ durable = rec.durable
    /\ flushed = rec.flushed
    /\ created_dirty = rec.created_dirty
    /\ next_write = rec.next_write
    /\ next_cache = rec.next_cache
    /\ crashed = rec.crashed
    /\ bad_read = rec.bad_read

NextMatches(rec) ==
    /\ visible' = rec.visible
    /\ write_store' = rec.write_store
    /\ write_hist' = rec.write_hist
    /\ dirty_q' = rec.dirty_q
    /\ cache_store' = rec.cache_store
    /\ durable' = rec.durable
    /\ flushed' = rec.flushed
    /\ created_dirty' = rec.created_dirty
    /\ next_write' = rec.next_write
    /\ next_cache' = rec.next_cache
    /\ crashed' = rec.crashed
    /\ bad_read' = rec.bad_read

WriterWriteAllowed ==
    \E k \in Keys, v \in Values :
        IF /\ ~crashed
           /\ next_write < MaxWrite
        THEN
            /\ write_store' =
                SetAt(write_store, WriteIx(next_write),
                    [present |-> TRUE, id |-> next_write, key |-> k, value |-> v])
            /\ write_hist' =
                SetAt(write_hist, WriteIx(next_write),
                    [present |-> TRUE, id |-> next_write, key |-> k, value |-> v])
            /\ dirty_q' = Append(dirty_q, next_write)
            /\ visible' = SetAt(visible, KeyIx(k), DirtyRef(next_write))
            /\ created_dirty' = SetAt(created_dirty, WriteIx(next_write), TRUE)
            /\ next_write' = next_write + 1
            /\ UNCHANGED << cache_store, durable, flushed, next_cache, crashed, bad_read >>
        ELSE
            NoOp

FlusherFlushNextAllowed ==
    IF /\ ~crashed
       /\ Len(dirty_q) > 0
    THEN
        LET rid == Head(dirty_q) IN
        LET wr == DirtyAt(write_hist, rid) IN
            IF wr.present
            THEN
                /\ durable' =
                    SetAt(durable, KeyIx(wr.key),
                        [present |-> TRUE, value |-> wr.value, seq |-> rid])
                /\ flushed' = Append(flushed, rid)
                /\ dirty_q' = Tail(dirty_q)
                /\ UNCHANGED << visible, write_store, write_hist, cache_store,
                                created_dirty, next_write, next_cache, crashed, bad_read >>
            ELSE
                NoOp
    ELSE
        NoOp

FlusherDropAllowed ==
    \E id \in WriteIds :
        IF /\ ~crashed
           /\ id \in SeqSet(flushed)
           /\ DirtyLive(write_store, id)
        THEN
            /\ write_store' = SetAt(write_store, WriteIx(id), NullDirty)
            /\ visible' = RemoveVisibleDirty(visible, id)
            /\ UNCHANGED << write_hist, dirty_q, cache_store, durable, flushed,
                            created_dirty, next_write, next_cache, crashed, bad_read >>
        ELSE
            NoOp

CacheInsertAllowed ==
    \E k \in Keys :
        IF /\ ~crashed
           /\ next_cache < MaxCache
           /\ DurableAt(durable, k).present
           /\ VisibleAt(visible, k) = NoneRef
        THEN
            /\ cache_store' =
                SetAt(cache_store, CacheIx(next_cache),
                    [present |-> TRUE, id |-> next_cache, key |-> k,
                     value |-> DurableAt(durable, k).value])
            /\ visible' = SetAt(visible, KeyIx(k), CleanRef(next_cache))
            /\ next_cache' = next_cache + 1
            /\ UNCHANGED << write_store, write_hist, dirty_q, durable, flushed,
                            created_dirty, next_write, crashed, bad_read >>
        ELSE
            NoOp

CacheDropRecordAllowed ==
    \E id \in CacheIds :
        IF /\ ~crashed
           /\ CleanLive(cache_store, id)
        THEN
            /\ cache_store' = SetAt(cache_store, CacheIx(id), NullClean)
            /\ UNCHANGED << visible, write_store, write_hist, dirty_q, durable,
                            flushed, created_dirty, next_write, next_cache,
                            crashed, bad_read >>
        ELSE
            NoOp

CacheCleanupVisibleAllowed ==
    \E id \in CacheIds :
        IF ~crashed
        THEN
            /\ visible' =
                IF ~CleanLive(cache_store, id)
                THEN RemoveVisibleClean(visible, id)
                ELSE visible
            /\ UNCHANGED << write_store, write_hist, dirty_q, cache_store, durable,
                            flushed, created_dirty, next_write, next_cache,
                            crashed, bad_read >>
        ELSE
            NoOp

ReaderStepAllowed ==
    \E k \in Keys :
        IF ~crashed
        THEN
            /\ bad_read' =
                IF VisibleAt(visible, k).kind = "Dirty"
                THEN
                    LET id == VisibleAt(visible, k).id IN
                        bad_read \/ ~DirtyLive(write_store, id)
                                 \/ (DirtyAt(write_store, id).key # k)
                ELSE IF VisibleAt(visible, k).kind = "Clean"
                THEN
                    LET id == VisibleAt(visible, k).id IN
                        bad_read \/ (CleanLive(cache_store, id)
                                     /\ (CleanAt(cache_store, id).key # k))
                ELSE
                    bad_read
            /\ UNCHANGED << visible, write_store, write_hist, dirty_q, cache_store, durable,
                            flushed, created_dirty, next_write, next_cache, crashed >>
        ELSE
            NoOp

CrashAllowed ==
    IF ~crashed
    THEN
        /\ visible' = [i \in 1..KeyCount |-> NoneRef]
        /\ write_store' = [i \in 1..MaxWrite |-> NullDirty]
        /\ dirty_q' = <<>>
        /\ cache_store' = [i \in 1..MaxCache |-> NullClean]
        /\ crashed' = TRUE
        /\ UNCHANGED << write_hist, durable, flushed, created_dirty,
                        next_write, next_cache, bad_read >>
    ELSE
        NoOp

TraceConstInit == Len(TraceLog) > 0

TraceInit ==
    /\ pos = 1
    /\ StateMatches(TraceLog[1])

TraceNext ==
    /\ pos < Len(TraceLog)
    /\ LET rec == TraceLog[pos + 1] IN
       /\ CASE rec.action = "WriterWrite" -> WriterWriteAllowed
          [] rec.action = "FlusherFlushNext" -> FlusherFlushNextAllowed
          [] rec.action = "FlusherDrop" -> FlusherDropAllowed
          [] rec.action = "CacheInsert" -> CacheInsertAllowed
          [] rec.action = "CacheDropRecord" -> CacheDropRecordAllowed
          [] rec.action = "CacheCleanupVisible" -> CacheCleanupVisibleAllowed
          [] rec.action = "ReaderStep" -> ReaderStepAllowed
          [] rec.action = "Crash" -> CrashAllowed
          [] OTHER -> FALSE
       /\ NextMatches(rec)
       /\ pos' = pos + 1

TraceFinished == pos < Len(TraceLog)

Spec == TraceConstInit /\ TraceInit /\ [][TraceNext]_vars

====
