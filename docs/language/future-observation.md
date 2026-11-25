# Future observation and delegation

This chapter defines ordinary Error observation responsibilities for source language `0.5`. It refines [asynchronous tasks](async.md) and [Errors](errors.md). MUST, MUST NOT, and MAY express requirements, prohibitions, and permitted implementation choices.

The rules preserve responsibility; they do not promise termination or observation within a finite time. A compiler MAY conservatively reject a program when it cannot establish the required facts. It is not required to accept every safe program.

## OBS01 Task identity and error responsibility

Each successfully admitted source async call returns a Future with a fresh task identity. Aliases, fields, collection elements, closure captures, and returned values MUST preserve that identity. Separate calls, including separate loop iterations at the same source location, have separate identities. Argument evaluation or startup validation that fails before returning a Future does not create a responsibility for that uncreated task.

`MayError(p)` means that an ordinary Error may escape task `p` through its body or required cleanup. A declared `throws` upper bound applies even when the current body has no failing path. Such a task is a producer, and its ordinary result carries an observation responsibility. Panic, traps, cancellation, shutdown, and OOM remain distinct from ordinary Errors.

An infallible outer Future can retain another producer in its successful result or captures. The inner responsibility remains separate. `Unit`, a `nothrow` factory, readiness, and handle unreachability MUST NOT by themselves discharge observation responsibilities.

## OBS02 Responsibility state

Describe each analysis path using the following abstract state. These symbols do not prescribe runtime fields or memory layouts.

| Symbol | Meaning                                                                         | Relevant transition                                                                |
| ------ | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `P`    | Unresolved responsibilities, identified by task and ordinary Error upper bound. | Direct observation, checked delegation, or an actual outward transfer.             |
| `D`    | Identities whose results have definitely been received by direct observation.   | Later cancellation or assignment does not revoke this fact.                        |
| `H`    | Retention paths through values, captures, or storage.                           | Retaining an identity does not itself establish observation.                       |
| `X`    | Responsibilities actually transferred at the current exit.                      | Attach them to a successful return, caller-reachable storage, or a valid delegate. |
| `R`    | Checked records connecting a producer to a receiver.                            | Only valid records can provisionally carry responsibility.                         |
| `G`    | Path facts, value alternatives, populations, and generations.                   | A merge must not promote a fact that fails for an applicable alternative.          |

For every producer already created on a continuing path, preserve the following condition:

```text
MayError(p) implies at least one of:
  p is in D;
  p is in P;
  a valid record in R carries p;
  p is in X.
```

Unresolved `P` entries are allowed during evaluation. At an exit or analysis stop, OBS10 applies. Conservative restoration can leave an entry in `P` while another receiver record still exists; the implementation need not represent these sets as disjoint.

## OBS03 Direct observation

Starting `await p` does not yet establish definite observation. When the successful value or ordinary Error is received and passed to the current task's result or Error route, add `p` to `D`. Catching or propagating that Error is a separate effect check.

If cancellation or shutdown interrupts a wait before the target result is received, that wait alone MUST NOT establish definite observation. A pending wait retains the target and its observation route. Physical release follows the [native lifetime rules](../abi/native-lifetime.md).

Repeated awaits and aliases refer to the same immutable completion. Receiving an inner Future as a successful outer result does not observe the inner Future. Deduplicating one Future's failures within a `try all` group does not observe a different task.

## OBS04 Retention and outward transfer

Assignment, copying, collection insertion, and closure construction update `H`; they MUST NOT by themselves add to `D`. Losing a retention path through overwrite, removal, or a Map key collision MUST NOT erase the producer's responsibility.

A successful return of a Future, aggregate, or closure transfers its attached responsibilities to the caller. A store into caller-reachable data must likewise describe the responsibilities retained by that data in the call contract. Unknown storage somewhere is neither observation nor a sufficient outward transfer.

If a failing finally cancels a pending return, transfer through that successful result did not occur. Responsibilities that no longer reach the caller must be handled or validly delegated on the failing path. Invoking a returned closure does not automatically observe a Future returned by that invocation.

`nothrow` bounds ordinary Errors escaping a call. The responsibility carried by the result of `fn () nothrow -> Future<T>` remains independent. Observation does not relax retention restrictions on Error payloads, native captures, or Worker transfer.

## OBS05 Delegation to a receiver

`R(r,p)` records receiver `r` carrying producer `p` provisionally. It is revocable and is not definite direct observation. A record must distinguish the producer, receiver, validity, incomplete versus completed state, direct observation versus further delegation, and mutable-path dependencies.

Responsibility MAY leave `P` provisionally only when all of these conditions hold:

| Condition                   | Requirement                                                                                                                                                                                                                            |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Identity                    | The applicable callee or callable alternatives and actual arguments identify the same producer. Discharge only identities common to every applicable alternative.                                                                      |
| Receiver paths              | Each applicable path directly observes the producer or delegates it to another valid receiver. Early return or an earlier ordinary Error that abandons the producer does not establish observation. Whole-process panic follows OBS10. |
| Blocking and nontermination | An await after a known nonreturning wait on another Future does not establish observation of this producer. Independent receivers follow OBS11.                                                                                        |
| Retention                   | The necessary Future, captures, and receiver environment remain retained by the responsible destination. Constructing a closure alone is insufficient.                                                                                 |
| Receiver's own failures     | If ordinary Errors can escape the receiver, its Future has its own responsibility. Infallibility and observing a producer are different properties.                                                                                    |
| Dependencies                | Preserve mutable paths, child receivers, and scope-control dependencies through subsequent operations and summaries.                                                                                                                   |

A synchronous helper that starts an async receiver does not complete that receiver by returning. Even when an infallible `Future<Unit>` receiver can be discarded, preserve its records for private producers and its possible control through aliases, returns, and summaries.

| Transition               | Precondition                                                               | Required effect                                                                                                                               |
| ------------------------ | -------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `Delegate(r,p)`          | OBS05 holds.                                                               | Create a valid incomplete `R(r,p)`. While it carries the responsibility, remove `p` from unresolved `P` if appropriate; do not add it to `D`. |
| `Invalidate(r,p)`        | OBS06 or OBS08 applies.                                                    | Invalidate the record and restore responsibility not already in `D`. Evaluate independent routes separately.                                  |
| `FinishDirect(r,p)`      | A valid record and the applicable completion establish direct observation. | Add `p` to `D`; it no longer relies on an incomplete provisional receiver.                                                                    |
| `FinishDelegated(r,q,p)` | Child receiver `q` still carries responsibility when `r` completes.        | Preserve `R(q,p)` and relevant control dependencies. Do not pretend that `q` completed or add `p` to `D` for that reason.                     |
| `Export(value,p)`        | The return or caller-reachable store actually succeeds.                    | Transfer `X` with the value and retain responsibility in the caller as unresolved or delegated.                                               |
| `RepeatAwait(r)`         | The same completion was already applied.                                   | Use the same result without reviving invalid records or replaying old stores.                                                                 |

Several records can refer to one producer; the table is not a single exclusive state enum.

### Coverage across receiver populations

Apply the coverage invariant to each producer. An operation on one producer's
records preserves the facts for other producers. An unaffected valid receiver
can continue carrying the same producer after revocation. Whenever revocation
leaves a producer outside `D`, `X`, and every valid receiver record, the producer
must remain or reappear in `P`. A failed outward transfer or an interrupted
wait preserves this coverage through the corresponding failing path.

## OBS06 Invalidation

| Event                                   | Required effect                                                                                                                                                             |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cancel or timeout a receiver            | Invalidate affected records whose completion is not established, and restore responsibilities carried only provisionally. The returned Bool does not establish observation. |
| Root shutdown or scope control          | Invalidate incomplete receiver records whose execution can be prevented by that control. Include requests through callees and native callbacks in call contracts.           |
| Control through an alias                | Track the target identity. If it is unknown, conservatively invalidate potentially affected records.                                                                        |
| Mutation of a dependency                | Invalidate affected path-dependent records according to OBS08.                                                                                                              |
| Return or repackage an invalid receiver | Do not make its record valid again. Reawaiting or obtaining an alias does not revive it.                                                                                    |
| Control after definite observation      | Do not revoke `D`. A receiver whose completion is established is no longer subject to invalidation based on being incomplete.                                               |

Invalidating one receiver does not imply that every receiver is invalid. An independent direct observation, another valid receiver, or a new checked delegation can satisfy the restored responsibility. If the compiler cannot establish that independence, it MAY restore `P` and require an explicit observation. An invalid field path or an older generation is not a substitute.

Static analysis MAY assume invalidation even when a runtime control request could return false. A guess that the receiver may already have completed is insufficient. This permits conservative rejection, not revocation of a definite observation.

## OBS07 Receiver completion and child delegation

When `r` completes and its result is received, only producers definitely directly observed on the applicable result path move to `D`. Responsibilities delegated to child `q` remain in `R(q,p)`. `await r` does not establish `q`'s completion.

Cancelling `q` later restores responsibility under OBS06. Reawaiting completed `r` MUST NOT revive the cancelled child's record. Conversely, cancelling already completed `r` alone MUST NOT cancel responsibility carried by an independent valid child receiver.

Before receiver execution establishes a child, preserve any parent-control dependency that can prevent that child from being established. Merely having a child is not sufficient to treat all parent and child control as identical.

## OBS08 Mutable fields and async writers

A receiver that will later read `cell.future` has a conditional delegation: the path must still select the relevant producer. An affecting store, mutation through an alias, unknown mutation, or outstanding writer that invalidates that fact revokes the path-dependent record.

A Future saved before the mutation, such as `let saved = cell.future`, is an independent value path. Observing it directly or delegating that value need not depend on later field stores. A store after direct observation is established by receiver completion does not restore that already observed responsibility.

An async writer's store MUST NOT be treated as complete at registration. Account for its applicable completion, intervening stores, and overlapping writers. Reawaiting a writer whose completion was already applied MUST NOT replay its old field state over current state.

## OBS09 Populations and generations

List and Map responsibilities refer to actual retained identities and populations. Claiming complete observation or delegation requires a completed traversal that covers every member. A break, a continue that skips a member, an earlier failure, a selected member, removal, overwrite, or dynamic key collision MUST NOT count as complete coverage.

Tasks created repeatedly at one producer site retain separate generation responsibilities. Do not reuse a previous batch's or partial subset's receipt for a new batch or the full population. Independent per-element receivers differ from sequential awaits when an earlier element may never return.

Concrete expansion limits, summary representations, and loop-invariant inference are implementation choices. Crossing a limit never permits identifying one element with a population or one generation with another.

## OBS10 Exits and responsibility checking

At normal return, fallthrough, and ordinary Error propagation, reject unresolved responsibility that cannot be attached to an actual outward transfer or valid receiver. Check each path including finally. An await on one path cannot discharge a different path's responsibility. `try all` does not implicitly adopt, observe, or join created tasks.

Apply the same check to analysis stops at known nonreturning waits or loops for responsibilities created before the stop. An unreachable later await or unchecked local retention does not excuse them. Responsibilities actually passed through a recursive contract follow OBS11.

Ordinary function return does not implicitly join every Future. Work may remain retained by the runtime while its transfers and delegations remain valid. Root completion and native settlement follow separate lifetime rules.

Whole-process panic paths MAY be excluded from observation checking for continuing ordinary exits. This does not relax retention checks in finally or required cancellation, joining, and environment retention. Arbitrary native crashes and forced termination do not guarantee cleanup.

## OBS11 Nontermination and implementation freedom

An async call registers a Future even when its body cannot return. A callee's nontermination MUST NOT erase later caller statements at registration or responsibilities already created before registration.

An observation after a known nonreturning wait is not an observation destination for an earlier producer. For example, a sequential receiver of `[forever(), fail()]` cannot claim to observe the second member. Registering independent receivers can avoid that dependency.

Recursive summaries must preserve the identities actually retained or transferred and receiver relationships affected by control. A recursive cycle alone is not completed observation. Accepting recursive delegation does not promise recursion termination or finite-time observation.

Alias inference, loop and recursion summaries, and expansion limits are implementation choices. They permit conservative rejection of unproved cases, not acceptance that violates responsibility preservation.

## Examples

Here `fail()` is an async producer and `receive(p)` asynchronously executes `try { await p; } catch errors {}`. `cancel` abbreviates `rinzu.future.cancel`. These are contextual fragments.

| Situation                                                         | Required interpretation                                                                                                           |
| ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `let p = fail(); receive(p);`                                     | A checked receiver can carry responsibility. If it has no escaping ordinary Error, its own Future need not be explicitly awaited. |
| `let p = fail(); let r = receive(p); cancel(r);`                  | Restore the producer's responsibility. Reject if no other checked observation or transfer satisfies it.                           |
| `let p = fail(); let r = receive(p); await r; cancel(r);`         | Do not restore responsibility whose direct observation is established by completion.                                              |
| `let r = fromCell(cell); cell.future = safe(); await r;`          | Do not discharge an old producer using an invalidated field dependency.                                                           |
| Await a parent, then cancel its returned child receiver.          | Parent completion does not establish child completion. Recheck the producer's responsibility.                                     |
| Await one member of a receiver population, then request shutdown. | Do not treat partial completion as completion of the population.                                                                  |
| `await writer; cell.future = safe(); await writer;`               | The second await does not replay the writer's old store.                                                                          |
