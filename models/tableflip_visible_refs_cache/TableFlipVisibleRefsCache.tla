------------------------------ MODULE TableFlipVisibleRefsCache ------------------------------
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
NullClean == [present |-> FALSE, key |-> DefaultKey, value |-> DefaultValue]
NullDurable == [present |-> FALSE, value |-> DefaultValue, seq |-> -1]

IsLiveDirty(r) == r.present
IsLiveClean(r) == r.present

ApplyOneDurable(store, wr) ==
    [store EXCEPT ![wr.key] = [present |-> TRUE, value |-> wr.value, seq |-> wr.id]]

RECURSIVE ApplyFlushed(_, _)
ApplyFlushed(flushed, writeHist) ==
    IF Len(flushed) = 0 THEN [k \in Keys |-> NullDurable]
    ELSE
        LET id == flushed[Len(flushed)] IN
        ApplyOneDurable(
            ApplyFlushed(SubSeq(flushed, 1, Len(flushed) - 1), writeHist),
            writeHist[id]
        )

SeqOrderedIds(s) ==
    \A i, j \in 1..Len(s) : i < j => s[i] < s[j]

VisibleHasDirty(visible, id) ==
    \E k \in Keys : visible[k] = DirtyRef(id)

VisibleHasClean(visible, id) ==
    \E k \in Keys : visible[k] = CleanRef(id)

IdsInSeq(s) == {s[i] : i \in 1..Len(s)}

(*--algorithm VisibleRefsCache
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
    crashed = FALSE;

define
    TypeOK ==
        /\ visible \in [Keys -> {[kind |-> "None", id |-> -1]}
                              \cup {DirtyRef(i) : i \in WriteIds}
                              \cup {CleanRef(i) : i \in CacheIds}]
        /\ writeStore \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
        /\ writeHist \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
        /\ dirtyQ \in Seq(WriteIds)
        /\ cacheStore \in [CacheIds -> [present : BOOLEAN, key : Keys, value : Values]]
        /\ durable \in [Keys -> [present : BOOLEAN, value : Values, seq : -1..(MaxWrite - 1)]]
        /\ flushed \in Seq(WriteIds)
        /\ createdDirty \subseteq WriteIds
        /\ nextWrite \in 0..MaxWrite
        /\ nextCache \in 0..MaxCache
        /\ crashed \in BOOLEAN

    DirtyRefsLive ==
        \A k \in Keys :
            visible[k].kind = "Dirty" =>
                /\ visible[k].id \in WriteIds
                /\ IsLiveDirty(writeStore[visible[k].id])

    CleanRefsLive ==
        \A k \in Keys :
            visible[k].kind = "Clean" =>
                /\ visible[k].id \in CacheIds
                /\ IsLiveClean(cacheStore[visible[k].id])

    DirtyQOrdered == SeqOrderedIds(dirtyQ)
    FlushedOrdered == SeqOrderedIds(flushed)
    DurableMatchesFlushed == durable = ApplyFlushed(flushed, writeHist)

    NoLostDirty ==
        crashed \/
        \A id \in createdDirty :
            (id \in IdsInSeq(dirtyQ)) \/ (id \in IdsInSeq(flushed)) \/
            VisibleHasDirty(visible, id) \/ IsLiveDirty(writeStore[id])
end define;

fair process Driver = 1
variable wk = DefaultKey, wv = DefaultValue, wid = 0;
begin
Loop:
    while TRUE do
        either
WriteDirty:
            with kk \in Keys, vv \in Values do
                if (~crashed) /\ (nextWrite < MaxWrite) then
                    writeStore[nextWrite] := [present |-> TRUE, id |-> nextWrite, key |-> kk, value |-> vv];
                    writeHist[nextWrite] := [present |-> TRUE, id |-> nextWrite, key |-> kk, value |-> vv];
                    dirtyQ := Append(dirtyQ, nextWrite);
                    visible[kk] := DirtyRef(nextWrite);
                    createdDirty := createdDirty \cup {nextWrite};
                    nextWrite := nextWrite + 1;
                end if;
            end with;
        or
FlushNext:
            if (~crashed) /\ (Len(dirtyQ) > 0) then
                wid := Head(dirtyQ);
                durable[writeHist[wid].key] := [present |-> TRUE, value |-> writeHist[wid].value, seq |-> wid];
                flushed := Append(flushed, wid);
                dirtyQ := Tail(dirtyQ);
            end if;
        or
DropFlushed:
            with ii \in WriteIds do
                if (~crashed) /\ (ii \in IdsInSeq(flushed)) /\ IsLiveDirty(writeStore[ii]) then
                    writeStore[ii] := NullDirty;
                    visible := [kk \in Keys |-> IF visible[kk] = DirtyRef(ii) THEN NoneRef ELSE visible[kk]];
                end if;
            end with;
        or
CacheInsert:
            with kk \in Keys do
                if (~crashed) /\ (nextCache < MaxCache) /\ durable[kk].present /\ (visible[kk].kind # "Dirty") /\ (visible[kk] = NoneRef) then
                    cacheStore[nextCache] := [present |-> TRUE, key |-> kk, value |-> durable[kk].value];
                    visible[kk] := CleanRef(nextCache);
                    nextCache := nextCache + 1;
                end if;
            end with;
        or
CacheEvict:
            with ii \in CacheIds do
                if (~crashed) /\ IsLiveClean(cacheStore[ii]) then
                    cacheStore[ii] := NullClean;
                    visible := [kk \in Keys |-> IF visible[kk] = CleanRef(ii) THEN NoneRef ELSE visible[kk]];
                end if;
            end with;
        or
Crash:
            if ~crashed then
                visible := [k \in Keys |-> NoneRef];
                writeStore := [i \in WriteIds |-> NullDirty];
                dirtyQ := <<>>;
                cacheStore := [i \in CacheIds |-> NullClean];
                crashed := TRUE;
            end if;
        end either;
    end while;
end process;
end algorithm; *)
\* BEGIN TRANSLATION (chksum(pcal) = "28e4ef50" /\ chksum(tla) = "2b773c7f")
VARIABLES visible, writeStore, writeHist, dirtyQ, cacheStore, durable, 
          flushed, createdDirty, nextWrite, nextCache, crashed, pc

(* define statement *)
TypeOK ==
    /\ visible \in [Keys -> {[kind |-> "None", id |-> -1]}
                          \cup {DirtyRef(i) : i \in WriteIds}
                          \cup {CleanRef(i) : i \in CacheIds}]
    /\ writeStore \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
    /\ writeHist \in [WriteIds -> [present : BOOLEAN, id : -1..(MaxWrite - 1), key : Keys, value : Values]]
    /\ dirtyQ \in Seq(WriteIds)
    /\ cacheStore \in [CacheIds -> [present : BOOLEAN, key : Keys, value : Values]]
    /\ durable \in [Keys -> [present : BOOLEAN, value : Values, seq : -1..(MaxWrite - 1)]]
    /\ flushed \in Seq(WriteIds)
    /\ createdDirty \subseteq WriteIds
    /\ nextWrite \in 0..MaxWrite
    /\ nextCache \in 0..MaxCache
    /\ crashed \in BOOLEAN

DirtyRefsLive ==
    \A k \in Keys :
        visible[k].kind = "Dirty" =>
            /\ visible[k].id \in WriteIds
            /\ IsLiveDirty(writeStore[visible[k].id])

CleanRefsLive ==
    \A k \in Keys :
        visible[k].kind = "Clean" =>
            /\ visible[k].id \in CacheIds
            /\ IsLiveClean(cacheStore[visible[k].id])

DirtyQOrdered == SeqOrderedIds(dirtyQ)
FlushedOrdered == SeqOrderedIds(flushed)
DurableMatchesFlushed == durable = ApplyFlushed(flushed, writeHist)

NoLostDirty ==
    crashed \/
    \A id \in createdDirty :
        (id \in IdsInSeq(dirtyQ)) \/ (id \in IdsInSeq(flushed)) \/
        VisibleHasDirty(visible, id) \/ IsLiveDirty(writeStore[id])

VARIABLES wk, wv, wid

vars == << visible, writeStore, writeHist, dirtyQ, cacheStore, durable, 
           flushed, createdDirty, nextWrite, nextCache, crashed, pc, wk, wv, 
           wid >>

ProcSet == {1}

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
        (* Process Driver *)
        /\ wk = DefaultKey
        /\ wv = DefaultValue
        /\ wid = 0
        /\ pc = [self \in ProcSet |-> "Loop"]

Loop == /\ pc[1] = "Loop"
        /\ \/ /\ pc' = [pc EXCEPT ![1] = "WriteDirty"]
           \/ /\ pc' = [pc EXCEPT ![1] = "FlushNext"]
           \/ /\ pc' = [pc EXCEPT ![1] = "DropFlushed"]
           \/ /\ pc' = [pc EXCEPT ![1] = "CacheInsert"]
           \/ /\ pc' = [pc EXCEPT ![1] = "CacheEvict"]
           \/ /\ pc' = [pc EXCEPT ![1] = "Crash"]
        /\ UNCHANGED << visible, writeStore, writeHist, dirtyQ, cacheStore, 
                        durable, flushed, createdDirty, nextWrite, nextCache, 
                        crashed, wk, wv, wid >>

WriteDirty == /\ pc[1] = "WriteDirty"
              /\ \E kk \in Keys:
                   \E vv \in Values:
                     IF (~crashed) /\ (nextWrite < MaxWrite)
                        THEN /\ writeStore' = [writeStore EXCEPT ![nextWrite] = [present |-> TRUE, id |-> nextWrite, key |-> kk, value |-> vv]]
                             /\ writeHist' = [writeHist EXCEPT ![nextWrite] = [present |-> TRUE, id |-> nextWrite, key |-> kk, value |-> vv]]
                             /\ dirtyQ' = Append(dirtyQ, nextWrite)
                             /\ visible' = [visible EXCEPT ![kk] = DirtyRef(nextWrite)]
                             /\ createdDirty' = (createdDirty \cup {nextWrite})
                             /\ nextWrite' = nextWrite + 1
                        ELSE /\ TRUE
                             /\ UNCHANGED << visible, writeStore, writeHist, 
                                             dirtyQ, createdDirty, nextWrite >>
              /\ pc' = [pc EXCEPT ![1] = "Loop"]
              /\ UNCHANGED << cacheStore, durable, flushed, nextCache, crashed, 
                              wk, wv, wid >>

FlushNext == /\ pc[1] = "FlushNext"
             /\ IF (~crashed) /\ (Len(dirtyQ) > 0)
                   THEN /\ wid' = Head(dirtyQ)
                        /\ durable' = [durable EXCEPT ![writeHist[wid'].key] = [present |-> TRUE, value |-> writeHist[wid'].value, seq |-> wid']]
                        /\ flushed' = Append(flushed, wid')
                        /\ dirtyQ' = Tail(dirtyQ)
                   ELSE /\ TRUE
                        /\ UNCHANGED << dirtyQ, durable, flushed, wid >>
             /\ pc' = [pc EXCEPT ![1] = "Loop"]
             /\ UNCHANGED << visible, writeStore, writeHist, cacheStore, 
                             createdDirty, nextWrite, nextCache, crashed, wk, 
                             wv >>

DropFlushed == /\ pc[1] = "DropFlushed"
               /\ \E ii \in WriteIds:
                    IF (~crashed) /\ (ii \in IdsInSeq(flushed)) /\ IsLiveDirty(writeStore[ii])
                       THEN /\ writeStore' = [writeStore EXCEPT ![ii] = NullDirty]
                            /\ visible' = [kk \in Keys |-> IF visible[kk] = DirtyRef(ii) THEN NoneRef ELSE visible[kk]]
                       ELSE /\ TRUE
                            /\ UNCHANGED << visible, writeStore >>
               /\ pc' = [pc EXCEPT ![1] = "Loop"]
               /\ UNCHANGED << writeHist, dirtyQ, cacheStore, durable, flushed, 
                               createdDirty, nextWrite, nextCache, crashed, wk, 
                               wv, wid >>

CacheInsert == /\ pc[1] = "CacheInsert"
               /\ \E kk \in Keys:
                    IF (~crashed) /\ (nextCache < MaxCache) /\ durable[kk].present /\ (visible[kk].kind # "Dirty") /\ (visible[kk] = NoneRef)
                       THEN /\ cacheStore' = [cacheStore EXCEPT ![nextCache] = [present |-> TRUE, key |-> kk, value |-> durable[kk].value]]
                            /\ visible' = [visible EXCEPT ![kk] = CleanRef(nextCache)]
                            /\ nextCache' = nextCache + 1
                       ELSE /\ TRUE
                            /\ UNCHANGED << visible, cacheStore, nextCache >>
               /\ pc' = [pc EXCEPT ![1] = "Loop"]
               /\ UNCHANGED << writeStore, writeHist, dirtyQ, durable, flushed, 
                               createdDirty, nextWrite, crashed, wk, wv, wid >>

CacheEvict == /\ pc[1] = "CacheEvict"
              /\ \E ii \in CacheIds:
                   IF (~crashed) /\ IsLiveClean(cacheStore[ii])
                      THEN /\ cacheStore' = [cacheStore EXCEPT ![ii] = NullClean]
                           /\ visible' = [kk \in Keys |-> IF visible[kk] = CleanRef(ii) THEN NoneRef ELSE visible[kk]]
                      ELSE /\ TRUE
                           /\ UNCHANGED << visible, cacheStore >>
              /\ pc' = [pc EXCEPT ![1] = "Loop"]
              /\ UNCHANGED << writeStore, writeHist, dirtyQ, durable, flushed, 
                              createdDirty, nextWrite, nextCache, crashed, wk, 
                              wv, wid >>

Crash == /\ pc[1] = "Crash"
         /\ IF ~crashed
               THEN /\ visible' = [k \in Keys |-> NoneRef]
                    /\ writeStore' = [i \in WriteIds |-> NullDirty]
                    /\ dirtyQ' = <<>>
                    /\ cacheStore' = [i \in CacheIds |-> NullClean]
                    /\ crashed' = TRUE
               ELSE /\ TRUE
                    /\ UNCHANGED << visible, writeStore, dirtyQ, cacheStore, 
                                    crashed >>
         /\ pc' = [pc EXCEPT ![1] = "Loop"]
         /\ UNCHANGED << writeHist, durable, flushed, createdDirty, nextWrite, 
                         nextCache, wk, wv, wid >>

Driver == Loop \/ WriteDirty \/ FlushNext \/ DropFlushed \/ CacheInsert
             \/ CacheEvict \/ Crash

Next == Driver

Spec == Init /\ [][Next]_vars

\* END TRANSLATION 

=============================================================================
