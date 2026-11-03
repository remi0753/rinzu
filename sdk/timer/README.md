# rinzu.timer

Timers are provided by the toolchain's `rinzu.timer` standard library. No native
declaration, manifest dependency, C library, or service identifier is needed.

```rinzu
import rinzu.timer;

async fn main() {
    try {
        await rinzu.timer.sleep(1000);
    }
    catch errors {
        panic("timer failed");
    }
}
```

`sleep(milliseconds: Int) -> Future<Unit>` waits for a nonnegative number of
milliseconds. Negative values throw a catchable Error. Zero still yields and
completes on a later runtime turn. Pending timers leave Main free to run other
tasks and do not consume CPU Worker admission.

The compiler supplies the library implementation and connects it to the common
reactor. Cancellation, shutdown, and physical completion use the existing Future
lifecycle. SDKs and applications use the same imported API.

[Async guide](../../docs/language/async.md) · [Standard libraries](../../docs/libraries/overview.md)
