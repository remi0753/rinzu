# Files, standard streams, and pull streams

`rinzu.io` provides bounded binary I/O. File handles belong to their acquiring Domain and physical Owner. Perform I/O on the supported Main path and explicitly close acquired resources after their users finish.

## Read or replace a whole file

`await readBytes(path, maxBytes)` reads a regular file up to the required byte limit. Zero accepts an empty file. A negative limit raises `InvalidReadLimit`, and a file beyond the limit raises `ReadLimitExceeded`. `readText` also checks the complete byte sequence for UTF-8 and raises `InvalidFileUtf8` with the decoder error as its cause. It preserves a BOM, NULs, and line endings.

`await replaceBytes(path, data)` and `await replaceText(path, text)` prepare and flush a sibling temporary file before atomically installing it. New files use mode `0600` subject to umask and inherited ACLs. Replacing an existing file preserves its owner, group, access mode, ACL, creation time where available, and macOS save-time extended attributes including quarantine. Failure to preserve required metadata stops the replacement at `CopyMetadata`.

`ReplaceReceipt.bytesWritten` is the size of the installed content. Check `cleanupErrors` after recording the installation. A `FileOperationFailed` describes the failing phase, the available native status and cause, and the sibling temporary name if cleanup could not remove it. `InstallUncertain` reports a rename whose outcome could not be established. Read the destination to check its current content before retrying.

Cancellation and timeouts wait for physical cleanup. A request observed before the installation decision prevents the rename. A request observed later can leave the file installed while the Future reports a control failure.

## Write a file

```rinzu-test
import rinzu.io;

async fn main() {
    try {
        let file = await rinzu.io.openWrite("message.bin");
        try {
            let written = await rinzu.io.writeAll(file, utf8Encode("Hello\n"));
            let flushed = await rinzu.io.flush(file);
            if written != 6 {
                panic("short write");
            }
        }
        finally {
            let closed = await rinzu.io.close(file);
        }
    }
    catch problems {
        panic("file write failed");
    }
}
```

Open modes are `openRead(path)` and `openWrite(path)`. A path is a nonempty String without NUL. Open itself is asynchronous and can fail. Put the cleanup scope immediately after acquisition.

## Read and write bytes

`await read(file,maximum)` returns `ReadChunk` with `data: ByteArray` and `eof: Bool`. Request between 1 and 65536 bytes. Consume actual returned bytes and use the EOF flag; a request does not promise to fill its buffer.

`await write(file,data)` can return a partial byte count. `writeAll` advances through partial writes. Individual native operations are bounded to 64 KiB; follow the public API's input limits rather than assuming arbitrary-size operations. Flush is normal stream flushing, not a crash-durability guarantee.

`standardInput()` and `standardOutput()` acquire owned File handles for binary pipes or terminals. Use byte APIs to preserve NUL and arbitrary binary content. For simple value display, use `rinzu.stdio`.

`rename(from,to)` and `remove(path)` are async filesystem operations. `monotonicMillis()` measures elapsed time; `unixMillis()` returns wall-clock time. Neither clock should be substituted for the other in deadlines.

## Pull a file in chunks

`rinzu.stream.openFile(path)` creates a FileStream. `openFileWithLimit(path,maximum)` chooses a chunk limit from 1 to 65536. Await `next(stream)` to get Option<ByteArray>; None marks EOF. Await `close(stream)` in finally.

Only one operation may use a particular stream at a time. This is a pull API: the caller determines backpressure by asking for the next chunk. There is no generic for-await syntax or push stream registration.

## Cleanup and cancellation

A cancelled operation must physically settle before its buffers or handle environment are released. Await active users before close. `closeOwnerFiles()` drains files belonging to the calling Owner, including acquisition paths that did not deliver their handle to source. Use it only when the application controls that Owner's complete resource scope.

`liveFiles`, `liveBuffers`, `liveOperations`, and `retainedControlBytes` help inspect SDK-managed quantities. A zero live-resource counter does not mean process RSS or all retained diagnostic controls are zero.

Detailed declarations: [rinzu.io](../../sdk/io/src/io.rinzu), [rinzu.stream](../../sdk/stream/src/stream.rinzu).
