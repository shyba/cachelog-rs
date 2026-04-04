---------------------------- MODULE TableFlipPrefixConsistent ----------------------------
EXTENDS Naturals, Integers, Sequences, FiniteSets, TLC

CONSTANTS Keys, Values, MaxSeq, EnablePromote

Null ==
    [seq |-> -1,
     key |-> CHOOSE k \in Keys : TRUE,
     value |-> CHOOSE v \in Values : TRUE,
     state |-> "Clean"]

EmptyMap == [k \in Keys |-> Null]

IsNull(e) == e.seq = -1

MakeEntry(seq, key, value, state) ==
    [seq |-> seq, key |-> key, value |-> value, state |-> state]

InSeq(r, s) == \E i \in 1..Len(s) : s[i] = r

InMap(r, m) == \E k \in Keys : m[k] = r

Entries(m) == {e \in {m[k] : k \in Keys} : ~IsNull(e)}

CleanEntries(m) == {e \in Entries(m) : e.state = "Clean"}

Visible(k, front, back, durable) ==
    IF ~IsNull(front[k]) THEN front[k]
    ELSE IF ~IsNull(back[k]) THEN back[k]
    ELSE durable[k]

RECURSIVE SetToSeqBySeq(_)
SetToSeqBySeq(S) ==
    IF S = {} THEN << >>
    ELSE
        LET least == CHOOSE e \in S : \A other \in S : e.seq <= other.seq
        IN <<least>> \o SetToSeqBySeq(S \ {least})

ApplyOne(store, r) == [store EXCEPT ![r.key] = r]

RECURSIVE ApplyFlushed(_)
ApplyFlushed(flushed) ==
    IF Len(flushed) = 0 THEN EmptyMap
    ELSE
        ApplyOne(
            ApplyFlushed(SubSeq(flushed, 1, Len(flushed) - 1)),
            flushed[Len(flushed)]
        )

SeqOrdered(s) ==
    \A i, j \in 1..Len(s) : i < j => s[i].seq < s[j].seq

VARIABLES front, back, dirtyQ, retiredClean, durable, flushed, createdDirty, nextSeq, crashed, restoreOverwriteError

Vars ==
    <<front, back, dirtyQ, retiredClean, durable, flushed, createdDirty, nextSeq, crashed, restoreOverwriteError>>

Init ==
    /\ front = EmptyMap
    /\ back = EmptyMap
    /\ dirtyQ = << >>
    /\ retiredClean = << >>
    /\ durable = EmptyMap
    /\ flushed = << >>
    /\ createdDirty = {}
    /\ nextSeq = 0
    /\ crashed = FALSE
    /\ restoreOverwriteError = FALSE

WriteDirty ==
    /\ ~crashed
    /\ nextSeq < MaxSeq
    /\ \E k \in Keys, v \in Values :
        LET r == MakeEntry(nextSeq, k, v, "Dirty")
        IN
            /\ front' = [front EXCEPT ![k] = r]
            /\ back' = back
            /\ dirtyQ' = Append(dirtyQ, r)
            /\ retiredClean' = retiredClean
            /\ durable' = durable
            /\ flushed' = flushed
            /\ createdDirty' = createdDirty \cup {r}
            /\ nextSeq' = nextSeq + 1
            /\ crashed' = crashed
            /\ restoreOverwriteError' = restoreOverwriteError

PromoteCleanFromBack ==
    /\ ~crashed
    /\ EnablePromote
    /\ \E k \in Keys :
        /\ IsNull(front[k])
        /\ ~IsNull(back[k])
        /\ front' = [front EXCEPT ![k] = [back[k] EXCEPT !.state = "Clean"]]
        /\ back' = back
        /\ dirtyQ' = dirtyQ
        /\ retiredClean' = retiredClean
        /\ durable' = durable
        /\ flushed' = flushed
        /\ createdDirty' = createdDirty
        /\ nextSeq' = nextSeq
        /\ crashed' = crashed
        /\ restoreOverwriteError' = restoreOverwriteError

Flip ==
    /\ ~crashed
    /\ retiredClean = << >>
    /\ front' = EmptyMap
    /\ back' = front
    /\ retiredClean' = SetToSeqBySeq(CleanEntries(back))
    /\ dirtyQ' = dirtyQ
    /\ durable' = durable
    /\ flushed' = flushed
    /\ createdDirty' = createdDirty
    /\ nextSeq' = nextSeq
    /\ crashed' = crashed
    /\ restoreOverwriteError' = restoreOverwriteError

FlushNext ==
    /\ ~crashed
    /\ Len(dirtyQ) > 0
    /\ LET r == Head(dirtyQ)
       IN
            /\ front' = front
            /\ back' = back
            /\ dirtyQ' = Tail(dirtyQ)
            /\ retiredClean' = retiredClean
            /\ durable' = ApplyOne(durable, r)
            /\ flushed' = Append(flushed, r)
            /\ createdDirty' = createdDirty
            /\ nextSeq' = nextSeq
            /\ crashed' = crashed
            /\ restoreOverwriteError' = restoreOverwriteError

FlushFail ==
    /\ ~crashed
    /\ Len(dirtyQ) > 0
    /\ LET r == Head(dirtyQ)
       IN
            /\ front' =
                IF IsNull(front[r.key]) \/ front[r.key].seq < r.seq
                THEN [front EXCEPT ![r.key] = r]
                ELSE front
            /\ back' = back
            /\ dirtyQ' = dirtyQ
            /\ retiredClean' = retiredClean
            /\ durable' = durable
            /\ flushed' = flushed
            /\ createdDirty' = createdDirty
            /\ nextSeq' = nextSeq
            /\ crashed' = crashed
            /\ restoreOverwriteError' =
                restoreOverwriteError \/
                (~IsNull(front[r.key]) /\ front[r.key].seq > r.seq /\ front' # front)

DrainClean ==
    /\ ~crashed
    /\ Len(retiredClean) > 0
    /\ front' = front
    /\ back' = back
    /\ dirtyQ' = dirtyQ
    /\ retiredClean' = Tail(retiredClean)
    /\ durable' = durable
    /\ flushed' = flushed
    /\ createdDirty' = createdDirty
    /\ nextSeq' = nextSeq
    /\ crashed' = crashed
    /\ restoreOverwriteError' = restoreOverwriteError

Crash ==
    /\ ~crashed
    /\ front' = EmptyMap
    /\ back' = EmptyMap
    /\ dirtyQ' = << >>
    /\ retiredClean' = << >>
    /\ durable' = durable
    /\ flushed' = flushed
    /\ createdDirty' = createdDirty
    /\ nextSeq' = nextSeq
    /\ crashed' = TRUE
    /\ restoreOverwriteError' = restoreOverwriteError

Next ==
    IF crashed THEN UNCHANGED Vars
    ELSE
        \/ WriteDirty
        \/ PromoteCleanFromBack
        \/ Flip
        \/ FlushNext
        \/ FlushFail
        \/ DrainClean
        \/ Crash

Spec == Init /\ [][Next]_Vars

FrontShadowsBack ==
    \A k \in Keys : ~IsNull(front[k]) => Visible(k, front, back, durable) = front[k]

BackShadowsDurable ==
    \A k \in Keys : IsNull(front[k]) /\ ~IsNull(back[k]) =>
        Visible(k, front, back, durable) = back[k]

DirtyQOrdered == SeqOrdered(dirtyQ)

FlushedOrdered == SeqOrdered(flushed)

DurableMatchesFlushed == durable = ApplyFlushed(flushed)

NoLostDirty ==
    crashed \/ \A r \in createdDirty :
        InSeq(r, dirtyQ) \/ InSeq(r, flushed) \/ InMap(r, front) \/ InMap(r, back)

RestoreDoesNotOverwriteNewerFront == ~restoreOverwriteError

PromotionPreservesDirtyObligation ==
    \A k \in Keys :
        ~IsNull(front[k]) /\ front[k].state = "Clean" /\
        ~IsNull(back[k]) /\ back[k].state = "Dirty"
            => back[k].seq <= front[k].seq

=============================================================================
