--------------------------- MODULE TableFlipVisibleRefsConcurrent ---------------------------
EXTENDS Naturals, Integers, Sequences, TLC

CONSTANTS Keys, Values, MaxWrite, MaxCache

DefaultKey == CHOOSE k \in Keys : TRUE
DefaultValue == CHOOSE v \in Values : TRUE

WriteIds == 0..(MaxWrite - 1)
CacheIds == 0..(MaxCache - 1)

NoneRef == [kind |-> "None", id |-> -1]
DirtyRef(i) == [kind |-> "Dirty", id |-> i]
CleanRef(i) == [kind |-> "Clean", id |-> i]

NullDirty == [present |-> FALSE, id |-> -1, key |-> DefaultKey, value |-> DefaultValue]
NullClean == [present |-> FALSE, id |-> -1, key |-> DefaultKey, value |-> DefaultValue]
NullDurable == [present |-> FALSE, value |-> DefaultValue, seq |-> -1]

IsLiveDirty(r) == r.present
IsLiveClean(r) == r.present

ApplyOneDurable(store, wr) ==
    [store EXCEPT ![wr.key] = [present |-> TRUE, value |-> wr.value, seq |-> wr.id]]

RECURSIVE ApplyFlushed(_, _)
ApplyFlushed(flushed, writeHist) ==
    IF Len(flushed) = 0 THEN [k \in Keys |-> NullDurable]
    ELSE
        LET rid == flushed[Len(flushed)] IN
        ApplyOneDurable(
            ApplyFlushed(SubSeq(flushed, 1, Len(flushed) - 1), writeHist),
            writeHist[rid]
        )

SeqOrderedIds(s) ==
    \A i, j \in 1..Len(s) : i < j => s[i] < s[j]

IdsInSeq(s) == {s[i] : i \in 1..Len(s)}

VisibleHasDirty(visible, rid) ==
    \E k \in Keys : visible[k] = DirtyRef(rid)

(*--algorithm VisibleRefsConcurrent
variables
    visible = [k \in Keys |-> NoneRef],
    writeStore = [i \in WriteIds |-> NullDirty],
    writeHist = [i \in WriteIds |-> NullDirty],
    dirtyQ = <<>>,
    cacheStore = [i \in CacheIds |-> NullClean],
    durable = [k \in Keys |-> NullDurable],
    flushed = <<>>,
    createdDirty = {},
    nextWrite = 0,
    nextCache = 0,
    crashed = FALSE,
    badRead = FALSE;

define
    TypeOK ==
        /\ visible \in [Keys -> {[kind |-> "None", id |-> -1]}
                              \cup {DirtyRef(i) : i \in WriteIds}
                              \cup {CleanRef(i) : i \in CacheIds}]
        /\ writeStore \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
        /\ writeHist \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
        /\ dirtyQ \in Seq(WriteIds)
        /\ cacheStore \in [CacheIds -> [present : BOOLEAN, id : -1..(MaxCache - 1), key : Keys, value : Values]]
        /\ durable \in [Keys -> [present : BOOLEAN, value : Values, seq : -1..(MaxWrite - 1)]]
        /\ flushed \in Seq(WriteIds)
        /\ createdDirty \subseteq WriteIds
        /\ nextWrite \in 0..MaxWrite
        /\ nextCache \in 0..MaxCache
        /\ crashed \in BOOLEAN
        /\ badRead \in BOOLEAN

    DirtyRefsLive ==
        \A k \in Keys :
            visible[k].kind = "Dirty" =>
                /\ visible[k].id \in WriteIds
                /\ IsLiveDirty(writeStore[visible[k].id])

    DirtyQOrdered == SeqOrderedIds(dirtyQ)
    FlushedOrdered == SeqOrderedIds(flushed)
    DurableMatchesFlushed == durable = ApplyFlushed(flushed, writeHist)

    NoLostDirty ==
        crashed \/
        \A rid \in createdDirty :
            (rid \in IdsInSeq(dirtyQ)) \/ (rid \in IdsInSeq(flushed)) \/
            VisibleHasDirty(visible, rid) \/ IsLiveDirty(writeStore[rid])

    NoBadRead == ~badRead
end define;

process Writer = 1
begin
WriterLoop:
    while TRUE do
WriterWrite:
        with k \in Keys, v \in Values do
            if (~crashed) /\ (nextWrite < MaxWrite) then
                writeStore[nextWrite] := [present |-> TRUE, id |-> nextWrite, key |-> k, value |-> v];
                writeHist[nextWrite] := [present |-> TRUE, id |-> nextWrite, key |-> k, value |-> v];
                dirtyQ := Append(dirtyQ, nextWrite);
                visible[k] := DirtyRef(nextWrite);
                createdDirty := createdDirty \cup {nextWrite};
                nextWrite := nextWrite + 1;
            end if;
        end with;
    end while;
end process;

process Flusher = 1
variable rid = 0;
begin
FlusherLoop:
    while TRUE do
        either
FlusherFlush:
            if (~crashed) /\ (Len(dirtyQ) > 0) then
                rid := Head(dirtyQ);
                durable[writeHist[rid].key] := [present |-> TRUE, value |-> writeHist[rid].value, seq |-> rid];
                flushed := Append(flushed, rid);
                dirtyQ := Tail(dirtyQ);
            end if;
        or
FlusherDrop:
            with id \in WriteIds do
                if (~crashed) /\ (id \in IdsInSeq(flushed)) /\ IsLiveDirty(writeStore[id]) then
                    writeStore[id] := NullDirty;
                    visible := [kk \in Keys |-> IF visible[kk] = DirtyRef(id) THEN NoneRef ELSE visible[kk]];
                end if;
            end with;
        end either;
    end while;
end process;

process CacheMaintainer = 1
begin
CacheLoop:
    while TRUE do
        either
CacheInsert:
            with k \in Keys do
                if (~crashed) /\ (nextCache < MaxCache) /\ durable[k].present /\ (visible[k] = NoneRef) then
                    cacheStore[nextCache] := [present |-> TRUE, id |-> nextCache, key |-> k, value |-> durable[k].value];
                    visible[k] := CleanRef(nextCache);
                    nextCache := nextCache + 1;
                end if;
            end with;
        or
CacheDropRecord:
            with id \in CacheIds do
                if (~crashed) /\ IsLiveClean(cacheStore[id]) then
                    cacheStore[id] := NullClean;
                end if;
            end with;
        or
CacheCleanupVisible:
            with id \in CacheIds do
                if (~crashed) then
                    visible := [kk \in Keys |-> IF (visible[kk] = CleanRef(id)) /\ ~IsLiveClean(cacheStore[id]) THEN NoneRef ELSE visible[kk]];
                end if;
            end with;
        end either;
    end while;
end process;

process Reader = 1
begin
ReaderLoop:
    while TRUE do
ReaderStep:
        with k \in Keys do
            if ~crashed then
                if visible[k].kind = "Dirty" then
                    if ~IsLiveDirty(writeStore[visible[k].id]) then
                        badRead := TRUE;
                    elsif writeStore[visible[k].id].key # k then
                        badRead := TRUE;
                    end if;
                elsif visible[k].kind = "Clean" then
                    if IsLiveClean(cacheStore[visible[k].id]) /\ (cacheStore[visible[k].id].key # k) then
                        badRead := TRUE;
                    end if;
                end if;
            end if;
        end with;
    end while;
end process;

process Crasher = 1
begin
CrasherLoop:
    while TRUE do
CrasherStep:
        if ~crashed then
            either
                skip;
            or
                visible := [k \in Keys |-> NoneRef];
                writeStore := [i \in WriteIds |-> NullDirty];
                dirtyQ := <<>>;
                cacheStore := [i \in CacheIds |-> NullClean];
                crashed := TRUE;
            end either;
        end if;
    end while;
end process;
end algorithm; *)
\* BEGIN TRANSLATION (chksum(pcal) = "621ee65a" /\ chksum(tla) = "e32dc3fa")
VARIABLES visible, writeStore, writeHist, dirtyQ, cacheStore, durable, 
          flushed, createdDirty, nextWrite, nextCache, crashed, badRead, pc

(* define statement *)
TypeOK ==
    /\ visible \in [Keys -> {[kind |-> "None", id |-> -1]}
                          \cup {DirtyRef(i) : i \in WriteIds}
                          \cup {CleanRef(i) : i \in CacheIds}]
    /\ writeStore \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
    /\ writeHist \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
    /\ dirtyQ \in Seq(WriteIds)
    /\ cacheStore \in [CacheIds -> [present : BOOLEAN, id : -1..(MaxCache - 1), key : Keys, value : Values]]
    /\ durable \in [Keys -> [present : BOOLEAN, value : Values, seq : -1..(MaxWrite - 1)]]
    /\ flushed \in Seq(WriteIds)
    /\ createdDirty \subseteq WriteIds
    /\ nextWrite \in 0..MaxWrite
    /\ nextCache \in 0..MaxCache
    /\ crashed \in BOOLEAN
    /\ badRead \in BOOLEAN

DirtyRefsLive ==
    \A k \in Keys :
        visible[k].kind = "Dirty" =>
            /\ visible[k].id \in WriteIds
            /\ IsLiveDirty(writeStore[visible[k].id])

DirtyQOrdered == SeqOrderedIds(dirtyQ)
FlushedOrdered == SeqOrderedIds(flushed)
DurableMatchesFlushed == durable = ApplyFlushed(flushed, writeHist)

NoLostDirty ==
    crashed \/
    \A rid \in createdDirty :
        (rid \in IdsInSeq(dirtyQ)) \/ (rid \in IdsInSeq(flushed)) \/
        VisibleHasDirty(visible, rid) \/ IsLiveDirty(writeStore[rid])

NoBadRead == ~badRead

VARIABLE rid

vars == << visible, writeStore, writeHist, dirtyQ, cacheStore, durable, 
           flushed, createdDirty, nextWrite, nextCache, crashed, badRead, pc, 
           rid >>

ProcSet == {1} \cup {1} \cup {1} \cup {1} \cup {1}

Init == (* Global variables *)
        /\ visible = [k \in Keys |-> NoneRef]
        /\ writeStore = [i \in WriteIds |-> NullDirty]
        /\ writeHist = [i \in WriteIds |-> NullDirty]
        /\ dirtyQ = <<>>
        /\ cacheStore = [i \in CacheIds |-> NullClean]
        /\ durable = [k \in Keys |-> NullDurable]
        /\ flushed = <<>>
        /\ createdDirty = {}
        /\ nextWrite = 0
        /\ nextCache = 0
        /\ crashed = FALSE
        /\ badRead = FALSE
        (* Process Flusher *)
        /\ rid = 0
        /\ pc = [self \in ProcSet |-> CASE self = 1 -> "WriterLoop"
                                        [] self = 1 -> "FlusherLoop"
                                        [] self = 1 -> "CacheLoop"
                                        [] self = 1 -> "ReaderLoop"
                                        [] self = 1 -> "CrasherLoop"]

WriterLoop == /\ pc[1] = "WriterLoop"
              /\ pc' = [pc EXCEPT ![1] = "WriterWrite"]
              /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                              cacheStore, durable, flushed, createdDirty, 
                              nextWrite, nextCache, crashed, badRead, rid >>

WriterWrite == /\ pc[1] = "WriterWrite"
               /\ \E k \in Keys:
                    \E v \in Values:
                      IF (~crashed) /\ (nextWrite < MaxWrite)
                         THEN /\ writeStore' = [writeStore EXCEPT ![nextWrite] = [present |-> TRUE, id |-> nextWrite, key |-> k, value |-> v]]
                              /\ writeHist' = [writeHist EXCEPT ![nextWrite] = [present |-> TRUE, id |-> nextWrite, key |-> k, value |-> v]]
                              /\ dirtyQ' = Append(dirtyQ, nextWrite)
                              /\ visible' = [visible EXCEPT ![k] = DirtyRef(nextWrite)]
                              /\ createdDirty' = (createdDirty \cup {nextWrite})
                              /\ nextWrite' = nextWrite + 1
                         ELSE /\ TRUE
                              /\ UNCHANGED << visible, writeStore, writeHist, 
                                              dirtyQ, createdDirty, nextWrite >>
               /\ pc' = [pc EXCEPT ![1] = "WriterLoop"]
               /\ UNCHANGED << cacheStore, durable, flushed, nextCache, 
                               crashed, badRead, rid >>

Writer == WriterLoop \/ WriterWrite

FlusherLoop == /\ pc[1] = "FlusherLoop"
               /\ \/ /\ pc' = [pc EXCEPT ![1] = "FlusherFlush"]
                  \/ /\ pc' = [pc EXCEPT ![1] = "FlusherDrop"]
               /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                               cacheStore, durable, flushed, createdDirty, 
                               nextWrite, nextCache, crashed, badRead, rid >>

FlusherFlush == /\ pc[1] = "FlusherFlush"
                /\ IF (~crashed) /\ (Len(dirtyQ) > 0)
                      THEN /\ rid' = Head(dirtyQ)
                           /\ durable' = [durable EXCEPT ![writeHist[rid'].key] = [present |-> TRUE, value |-> writeHist[rid'].value, seq |-> rid']]
                           /\ flushed' = Append(flushed, rid')
                           /\ dirtyQ' = Tail(dirtyQ)
                      ELSE /\ TRUE
                           /\ UNCHANGED << dirtyQ, durable, flushed, rid >>
                /\ pc' = [pc EXCEPT ![1] = "FlusherLoop"]
                /\ UNCHANGED << visible, writeStore, writeHist, cacheStore, 
                                createdDirty, nextWrite, nextCache, crashed, 
                                badRead >>

FlusherDrop == /\ pc[1] = "FlusherDrop"
               /\ \E id \in WriteIds:
                    IF (~crashed) /\ (id \in IdsInSeq(flushed)) /\ IsLiveDirty(writeStore[id])
                       THEN /\ writeStore' = [writeStore EXCEPT ![id] = NullDirty]
                            /\ visible' = [kk \in Keys |-> IF visible[kk] = DirtyRef(id) THEN NoneRef ELSE visible[kk]]
                       ELSE /\ TRUE
                            /\ UNCHANGED << visible, writeStore >>
               /\ pc' = [pc EXCEPT ![1] = "FlusherLoop"]
               /\ UNCHANGED << writeHist, dirtyQ, cacheStore, durable, flushed, 
                               createdDirty, nextWrite, nextCache, crashed, 
                               badRead, rid >>

Flusher == FlusherLoop \/ FlusherFlush \/ FlusherDrop

CacheLoop == /\ pc[1] = "CacheLoop"
             /\ \/ /\ pc' = [pc EXCEPT ![1] = "CacheInsert"]
                \/ /\ pc' = [pc EXCEPT ![1] = "CacheDropRecord"]
                \/ /\ pc' = [pc EXCEPT ![1] = "CacheCleanupVisible"]
             /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                             cacheStore, durable, flushed, createdDirty, 
                             nextWrite, nextCache, crashed, badRead, rid >>

CacheInsert == /\ pc[1] = "CacheInsert"
               /\ \E k \in Keys:
                    IF (~crashed) /\ (nextCache < MaxCache) /\ durable[k].present /\ (visible[k] = NoneRef)
                       THEN /\ cacheStore' = [cacheStore EXCEPT ![nextCache] = [present |-> TRUE, id |-> nextCache, key |-> k, value |-> durable[k].value]]
                            /\ visible' = [visible EXCEPT ![k] = CleanRef(nextCache)]
                            /\ nextCache' = nextCache + 1
                       ELSE /\ TRUE
                            /\ UNCHANGED << visible, cacheStore, nextCache >>
               /\ pc' = [pc EXCEPT ![1] = "CacheLoop"]
               /\ UNCHANGED << writeStore, writeHist, dirtyQ, durable, flushed, 
                               createdDirty, nextWrite, crashed, badRead, rid >>

CacheDropRecord == /\ pc[1] = "CacheDropRecord"
                   /\ \E id \in CacheIds:
                        IF (~crashed) /\ IsLiveClean(cacheStore[id])
                           THEN /\ cacheStore' = [cacheStore EXCEPT ![id] = NullClean]
                           ELSE /\ TRUE
                                /\ UNCHANGED cacheStore
                   /\ pc' = [pc EXCEPT ![1] = "CacheLoop"]
                   /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                                   durable, flushed, createdDirty, nextWrite, 
                                   nextCache, crashed, badRead, rid >>

CacheCleanupVisible == /\ pc[1] = "CacheCleanupVisible"
                       /\ \E id \in CacheIds:
                            IF (~crashed)
                               THEN /\ visible' = [kk \in Keys |-> IF (visible[kk] = CleanRef(id)) /\ ~IsLiveClean(cacheStore[id]) THEN NoneRef ELSE visible[kk]]
                               ELSE /\ TRUE
                                    /\ UNCHANGED visible
                       /\ pc' = [pc EXCEPT ![1] = "CacheLoop"]
                       /\ UNCHANGED << writeStore, writeHist, dirtyQ, 
                                       cacheStore, durable, flushed, 
                                       createdDirty, nextWrite, nextCache, 
                                       crashed, badRead, rid >>

CacheMaintainer == CacheLoop \/ CacheInsert \/ CacheDropRecord
                      \/ CacheCleanupVisible

ReaderLoop == /\ pc[1] = "ReaderLoop"
              /\ pc' = [pc EXCEPT ![1] = "ReaderStep"]
              /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                              cacheStore, durable, flushed, createdDirty, 
                              nextWrite, nextCache, crashed, badRead, rid >>

ReaderStep == /\ pc[1] = "ReaderStep"
              /\ \E k \in Keys:
                   IF ~crashed
                      THEN /\ IF visible[k].kind = "Dirty"
                                 THEN /\ IF ~IsLiveDirty(writeStore[visible[k].id])
                                            THEN /\ badRead' = TRUE
                                            ELSE /\ IF writeStore[visible[k].id].key # k
                                                       THEN /\ badRead' = TRUE
                                                       ELSE /\ TRUE
                                                            /\ UNCHANGED badRead
                                 ELSE /\ IF visible[k].kind = "Clean"
                                            THEN /\ IF IsLiveClean(cacheStore[visible[k].id]) /\ (cacheStore[visible[k].id].key # k)
                                                       THEN /\ badRead' = TRUE
                                                       ELSE /\ TRUE
                                                            /\ UNCHANGED badRead
                                            ELSE /\ TRUE
                                                 /\ UNCHANGED badRead
                      ELSE /\ TRUE
                           /\ UNCHANGED badRead
              /\ pc' = [pc EXCEPT ![1] = "ReaderLoop"]
              /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                              cacheStore, durable, flushed, createdDirty, 
                              nextWrite, nextCache, crashed, rid >>

Reader == ReaderLoop \/ ReaderStep

CrasherLoop == /\ pc[1] = "CrasherLoop"
               /\ pc' = [pc EXCEPT ![1] = "CrasherStep"]
               /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, 
                               cacheStore, durable, flushed, createdDirty, 
                               nextWrite, nextCache, crashed, badRead, rid >>

CrasherStep == /\ pc[1] = "CrasherStep"
               /\ IF ~crashed
                     THEN /\ \/ /\ TRUE
                                /\ UNCHANGED <<visible, writeStore, dirtyQ, cacheStore, crashed>>
                             \/ /\ visible' = [k \in Keys |-> NoneRef]
                                /\ writeStore' = [i \in WriteIds |-> NullDirty]
                                /\ dirtyQ' = <<>>
                                /\ cacheStore' = [i \in CacheIds |-> NullClean]
                                /\ crashed' = TRUE
                     ELSE /\ TRUE
                          /\ UNCHANGED << visible, writeStore, dirtyQ, 
                                          cacheStore, crashed >>
               /\ pc' = [pc EXCEPT ![1] = "CrasherLoop"]
               /\ UNCHANGED << writeHist, durable, flushed, createdDirty, 
                               nextWrite, nextCache, badRead, rid >>

Crasher == CrasherLoop \/ CrasherStep

Next == Writer \/ Flusher \/ CacheMaintainer \/ Reader \/ Crasher

Spec == Init /\ [][Next]_vars

\* END TRANSLATION 

=============================================================================
