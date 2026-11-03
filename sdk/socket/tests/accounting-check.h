/* Independent table-scan oracle. Production counters are never used to derive
   the expected values. Call only from tests, outside the SDK lock. */
#include <assert.h>

static void check_accounting(void) {
  pthread_mutex_lock(&lock);
  size_t actual_handles = 0, actual_operations = 0, actual_buffers = 0, actual_bytes = 0;
  size_t actual_pending = 0;
  for (size_t i = 0; i < HANDLES; ++i) {
    struct Handle *h = &handles[i];
    bool closing = (closing_handles[i / 64] >> (i % 64)) & 1;
    assert(closing == (h->id && h->closing && h->fd >= 0));
    if (!h->id)
      continue;
    ++actual_handles;
    unsigned refs = 0, reads = 0, writes = 0;
    for (size_t j = 0; j < OPS; ++j) {
      struct Operation *op = &operations[j];
      if (!op->id)
        continue;
      refs += op->handle == h;
      refs += op->value == h && op->handle != h;
      if (op->handle == h) {
        reads += op->kind == READ || op->kind == ACCEPT;
        writes += op->kind == WRITE || op->kind == HALF;
      }
    }
    assert(h->refs == refs && h->reads == reads && h->writes == writes);
  }
  for (size_t i = 0; i < OPS; ++i) {
    struct Operation *op = &operations[i];
    bool pending = (pending_operations[i / 64] >> (i % 64)) & 1;
    assert(pending == (op->id && !op->ready));
    if (!op->id)
      continue;
    ++actual_operations;
    actual_pending += !op->ready;
    actual_buffers += op->buffer != NULL;
    actual_bytes += op->capacity;
    assert((i < DATA_OPS) == (op->kind != CLOSE));
  }
  assert(handle_count == actual_handles && operation_count == actual_operations);
  assert(pending_count == actual_pending);
  assert(buffers == actual_buffers && buffer_bytes == actual_bytes);
  size_t accounted_handles = 0, accounted_operations = 0, accounted_bytes = 0;
  for (size_t i = 0; i < HANDLES; ++i) {
    struct Owner *account = &owners[i];
    if (!account->handles && !account->operations) {
      assert(!account->data_operations && !account->bytes);
      continue;
    }
    size_t n_handles = 0, n_operations = 0, n_data = 0, n_bytes = 0;
    for (size_t j = 0; j < HANDLES; ++j)
      if (handles[j].id && pthread_equal(handles[j].owner, account->thread)) {
        ++n_handles;
        assert(handles[j].account == account);
      }
    for (size_t j = 0; j < OPS; ++j)
      if (operations[j].id && pthread_equal(operations[j].owner, account->thread)) {
        ++n_operations;
        n_data += j < DATA_OPS;
        n_bytes += operations[j].capacity;
        assert(operations[j].account == account);
      }
    assert(account->handles == n_handles && account->operations == n_operations &&
           account->data_operations == n_data && account->bytes == n_bytes);
    accounted_handles += n_handles;
    accounted_operations += n_operations;
    accounted_bytes += n_bytes;
  }
  assert(accounted_handles == actual_handles && accounted_operations == actual_operations &&
         accounted_bytes == actual_bytes);
  pthread_mutex_unlock(&lock);
}
