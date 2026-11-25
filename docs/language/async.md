# Asynchronous tasks and Workers

Calling an async function queues its body and returns `Future<T>`. The task starts when selected by the runtime. Await is available inside async functions and observes a Future's result or ordinary failure.

```rinzu-test
import rinzu.core;
import rinzu.timer;

async fn later(value: Int) -> Int {
    await rinzu.timer.sleep(1);
    return value;
}

fn score(value: Int) -> Int {
    return value * 2;
}

async fn main() {
    try {
        let pending = later(21);
        let value = await pending;
        let result = await spawn score(value);
        if result != 42 {
            rinzu.core.panic("wrong async result");
        }
    }
    catch problems {
        rinzu.core.panic("async work failed");
    }
}
```

## Async is cooperative

Async work normally advances in the Main Domain. A pending timer or I/O operation returns control to the runtime. Calling an async function does not create a thread, and its body does not run inline at the call site. Avoid long CPU loops on Main; use a Worker for synchronous CPU work.

A Future's completion is stable. Repeated await observes the same completion. A caller does not implicitly join every Future at the end of its function. Main's natural shutdown waits for accepted work and required physical settlement.

## Observe possible failures

The compiler tracks responsibility for a Future that can produce an ordinary Error, including aliases, fields, collections, returns, and closure captures. Await it, return it to a responsible caller, or hand responsibility to code whose checked contract preserves or discharges that responsibility. A Future can be discarded only when ordinary failure cannot escape it, a property inferred by the compiler; responsibilities retained in its result or captures remain separate.

Delegation is provisional. Cancelling an unfinished observer or changing a field on which it depends can restore the original responsibility. Completing a parent does not complete its unawaited observers. This check does not promise termination or observation within a finite time. See [Future observation and delegation](future-observation.md) for the normative rules.

Unit results and readiness are not proof that failure is impossible. A `try all` block does not automatically observe tasks created inside it. When an outer Future returns data containing another Future, observing the outer result does not observe the inner Future.

## Execute CPU work with spawn

`spawn function(arguments)` runs a synchronous function in a separate Worker Domain. Its arguments and result must be Transferable. The function must handle ordinary Errors before its Worker exit. Transfer creates snapshots; mutable classes, function values, native handles, Error, and Future cannot cross this boundary.

Workers do not expose shared mutable memory, executor selection, or a public worker-count API. General async Worker functions are unavailable. A native API can impose stricter physical Owner or Main-thread requirements.

## Cancellation and shutdown

`rinzu.future.isReady(future)` checks readiness. `rinzu.future.cancel(future)` and `rinzu.future.timeout(future)` request the corresponding completion behavior for supported Main Futures. Completed results are not overwritten. Cancellation still waits for native and child work to physically settle before releasing their environments.

The Bool returned by cancel or timeout reports whether the runtime accepted a new control request. It does not confirm physical completion. A later request for an already controlled or completed Future returns false. SDK publication, runtime collection, helper return, and environment release are separate conditions; see [Native lifetime during cancellation](../abi/native-lifetime.md) for the normative state machines.

`rinzu.shutdown.request()` requests cooperative Root shutdown. To set an exit status, call `rinzu.process.setExitStatus(0..255)` before requesting shutdown. Cleanup failure or runtime failure still produces a failure status.

For signal-driven applications, use `rinzu.shutdown.enableSignals`, poll `signalRequested`, request shutdown, and disable signals in finally. Stop accepting new work, await outstanding users, then close their resources. A cancellation request is not evidence that a native call has returned.

Next: [Native interoperability](native-interop.md).
