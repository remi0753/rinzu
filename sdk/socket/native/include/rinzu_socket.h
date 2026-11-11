#ifndef RINZU_SOCKET_H
#define RINZU_SOCKET_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "rinzu/native_bytes.h"
/* Start returns a positive operation ID, or negative SDK status without admission.
   Timeout 0 is untimed; positive 1..30000 is an absolute monotonic operation deadline. */
enum RinzuSocketStatus {
  RINZU_SOCKET_OWNER = -7501,
  RINZU_SOCKET_CLOSED = -7502,
  RINZU_SOCKET_BUSY = -7503,
  RINZU_SOCKET_ARGUMENT = -7504,
  RINZU_SOCKET_SATURATED = -7505,
  RINZU_SOCKET_UNSUPPORTED = -7506,
  RINZU_SOCKET_SEND_CLOSED = -7507,
  RINZU_SOCKET_NO_PROGRESS = -7508,
  RINZU_SOCKET_DEADLINE = -7509
};
enum RinzuSocketStatistic {
  RINZU_SOCKET_LISTENERS,
  RINZU_SOCKET_STREAMS,
  RINZU_SOCKET_HANDLE_SLOTS,
  RINZU_SOCKET_OPERATIONS,
  RINZU_SOCKET_DATA_OPERATIONS,
  RINZU_SOCKET_CONTROL_OPERATIONS,
  RINZU_SOCKET_BUFFERS,
  RINZU_SOCKET_BUFFER_BYTES,
  RINZU_SOCKET_PEAK_BUFFER_BYTES,
  RINZU_SOCKET_CONTROL_BYTES,
  RINZU_SOCKET_RETAINED_CONTROL_BYTES,
  RINZU_SOCKET_EVENT_THREADS,
  RINZU_SOCKET_KEVENT_CALLS,
  RINZU_SOCKET_READINESS_EVENTS,
  RINZU_SOCKET_USER_WAKES,
  RINZU_SOCKET_POLL_TIMEOUTS,
  RINZU_SOCKET_STALE_EVENTS,
  RINZU_SOCKET_READ_CALLS,
  RINZU_SOCKET_WRITE_CALLS,
  RINZU_SOCKET_PARTIAL_WRITES,
  RINZU_SOCKET_COPY_BYTES,
  RINZU_SOCKET_ALLOCATIONS,
  RINZU_SOCKET_RECEIVED_BYTES,
  RINZU_SOCKET_SENT_BYTES,
  RINZU_SOCKET_PENDING_OPERATIONS,
  RINZU_SOCKET_FINISH_WAIT_OPERATIONS
};
int32_t rinzu_socket_address_status(const char *host, int32_t port);
int64_t rinzu_socket_listen(const char *host, int32_t port, int32_t backlog);
int64_t rinzu_socket_connect(const char *host, int32_t port, int64_t timeout);
int64_t rinzu_socket_accept(void *listener, int64_t timeout);
int64_t rinzu_socket_read(void *stream, int64_t maximum, int64_t timeout);
/* Private Source bridge. A successful ready read uses slot 1023 in the low
   ten bits of its generation token. It has no completion carrier and must
   end in the current Owner turn. Other positive values are operation IDs. */
int64_t rinzu_socket_read_start(void *stream, int64_t maximum, int64_t timeout);
void rinzu_socket_end_ready_read(int64_t id, bool delivered);
int64_t rinzu_socket_write(void *stream, const uint8_t *data, size_t length, bool all,
                           int64_t timeout);
/* Private Source bridge. Positive values are operation IDs. Immediate success
   returns -65536 minus the sent length, below the negative admission statuses.
   Pending writes copy only the caller's unsent bytes. */
enum { RINZU_SOCKET_WRITE_READY_BASE = -65536 };
int64_t rinzu_socket_write_start(void *stream, const uint8_t *data, size_t length, bool all,
                                 int64_t timeout);
/* One nonblocking send. Uses the ready receipt encoding for the accepted
   prefix (including zero). It never admits an operation or retains data. */
int64_t rinzu_socket_try_write(void *stream, const uint8_t *data, size_t length);
/* The shared physically complete marker carries no operation or byte count. */
void *rinzu_socket_write_ready(void);
int64_t rinzu_socket_shutdown(void *stream);
int64_t rinzu_socket_close(void *handle);
int64_t rinzu_socket_close_listener(void *listener);
int64_t rinzu_socket_close_stream(void *stream);
void *rinzu_socket_operation(int64_t id);
void *rinzu_socket_completion(void *operation);
void *rinzu_socket_transfer_completion(int64_t id);
const rinzu_native_byte_array_t *rinzu_socket_bytes_view_id(int64_t id);
void *rinzu_socket_borrow_read(int64_t id);
void *rinzu_socket_read_access(void);
int64_t rinzu_socket_read_length(void *view);
int64_t rinzu_socket_read_byte(void *view, int64_t index);
int64_t rinzu_socket_find_read_byte(void *view, int64_t byte, int64_t start, int64_t end);
const rinzu_native_byte_array_t *rinzu_socket_copy_read(void *view, int64_t start, int64_t end);
void rinzu_socket_end_read(int64_t id);
void rinzu_socket_ack_id(int64_t id);
void rinzu_socket_cancel_id(int64_t id);
void rinzu_socket_finish_id(int64_t id);
void *rinzu_socket_value(void *operation);
void *rinzu_socket_listener_value(void *operation);
void *rinzu_socket_stream_value(void *operation);
int32_t rinzu_socket_status(void *operation);
int64_t rinzu_socket_length(void *operation);
/* Private READ/WRITE bridge after physical completion. The signed CInt status
   and 0..65536 transferred bytes fit one Int: status * 131072 + length.
   The low 17 bits hold length, including a failed write's transferred prefix. */
int64_t rinzu_socket_transfer_result(void *operation);
/* Exact-length result view. Descriptor and data stay pinned until finish;
   no padding is exposed. The managed receiver copies once before returning. */
const rinzu_native_byte_array_t *rinzu_socket_bytes_view(void *operation);
/* Read buffer has 65536 initialized bytes; pinned through every completion await and finish. */
const uint8_t *rinzu_socket_bytes(void *operation);
/* Private fixed-prefix bridges. Length must fit; the same initialized buffer
   and completion/finish pin apply. The full-buffer getter stays compatible. */
const uint8_t *rinzu_socket_bytes_256(void *operation);
const uint8_t *rinzu_socket_bytes_4096(void *operation);
const uint8_t *rinzu_socket_bytes_8192(void *operation);
const uint8_t *rinzu_socket_bytes_16384(void *operation);
const uint8_t *rinzu_socket_bytes_32768(void *operation);
void rinzu_socket_ack(void *operation);
void rinzu_socket_cancel(void *operation);
void rinzu_socket_finish(void *operation);
bool rinzu_socket_join_idle(void);
int64_t rinzu_socket_port(void *listener);
bool rinzu_socket_owner_has_handle(void);
void *rinzu_socket_owner_handle(void);
bool rinzu_socket_owner_has_listener(void);
bool rinzu_socket_owner_has_stream(void);
void *rinzu_socket_owner_listener(void);
void *rinzu_socket_owner_stream(void);
int64_t rinzu_socket_live_handles(void);
int64_t rinzu_socket_live_operations(void);
int64_t rinzu_socket_live_buffers(void);
int64_t rinzu_socket_partial_writes(void);
int64_t rinzu_socket_event_count(void);
int64_t rinzu_socket_thread_count(void);
int64_t rinzu_socket_stale_events(void);
int64_t rinzu_socket_blocked_writes(void);
int64_t rinzu_socket_stat(int32_t statistic);
#endif
