#include <sqlite3.h>
#include <pthread.h>
#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
static bool fail_allocation, copied, losing;
static unsigned released;
static void *selected_allocate(sqlite3_uint64 size) {
  return fail_allocation ? NULL : sqlite3_malloc64(size);
}
static void selected_free(void *data) {
  assert(copied || losing);
  released++;
  sqlite3_free(data);
}
#define sqlite3_malloc64 selected_allocate
#define sqlite3_free selected_free
#include "../native/src/sqlite_source.c"
#undef sqlite3_malloc64
#undef sqlite3_free
static void *foreign(void *value) {
  assert(rinzu_sqlite_owned_release(value) == RINZU_SQLITE_OWNER);
  assert(rinzu_sqlite_owned_begin(value, "SELECT x'00'", false) == RINZU_SQLITE_OWNER);
  return NULL;
}
static int32_t reentry(void *value) {
  assert(rinzu_sqlite_owned_begin(value, "SELECT x'00'", false) == RINZU_SQLITE_REENTRY);
  return 0;
}
int main(int argc, char **argv) {
  (void)argv;
  sqlite3_int64 baseline = sqlite3_memory_used();
  void *connection = rinzu_sqlite_open(":memory:");
  assert(rinzu_sqlite_connection_status(connection) == 0);
  if (argc > 1) {
    fail_allocation = true;
    rinzu_sqlite_owned_begin(connection, "SELECT x'00'", false);
    return 1;
  }
  for (unsigned i = 0; i < 128; i++) {
    copied = false;
    losing = false;
    assert(rinzu_sqlite_owned_begin(connection, "SELECT x'00ff0001'", false) == 0);
    assert(rinzu_sqlite_owned_live() == 1);
    assert(rinzu_sqlite_owned_length(connection) == 4);
    assert(rinzu_sqlite_close(connection) == RINZU_SQLITE_IN_FLIGHT);
    assert(rinzu_sqlite_owned_begin(connection, "SELECT x'00'", false) == RINZU_SQLITE_REENTRY);
    pthread_t other;
    assert(!pthread_create(&other, NULL, foreign, connection));
    assert(!pthread_join(other, NULL));
    uint8_t snapshot[4];
    memcpy(snapshot, rinzu_sqlite_owned_bytes(connection), sizeof(snapshot));
    copied = true;
    assert(rinzu_sqlite_owned_release(connection) == 0);
    assert(snapshot[1] == 255);
    assert(rinzu_sqlite_owned_release(connection) == RINZU_SQLITE_NO_RESULT);
    assert(rinzu_sqlite_owned_live() == 0);
  }
  copied = false;
  losing = true;
  assert(rinzu_sqlite_owned_begin(connection, "SELECT cast(x'ff' AS TEXT)", true) == 0);
  assert(rinzu_sqlite_owned_release(connection) == 0);
  assert(rinzu_sqlite_owned_begin(connection, "SELECT zeroblob(65537)", false) ==
         RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_owned_begin(connection, "SELECT 1", false) == RINZU_SQLITE_TYPE);
  assert(rinzu_sqlite_owned_begin(connection, "SELECT x'' WHERE 0", false) == RINZU_SQLITE_ROW);
  assert(rinzu_sqlite_owned_begin(connection, "SELECT x'00'; SELECT 1", false) ==
         RINZU_SQLITE_SINGLE_STATEMENT);
  assert(rinzu_sqlite_observe(connection, "SELECT 1", connection, reentry) == 0);
  assert(rinzu_sqlite_close(connection) == 0);
  assert(rinzu_sqlite_owned_allocations() == 129 && rinzu_sqlite_owned_releases() == 129 &&
         released == 129 && rinzu_sqlite_owned_live() == 0);
  assert(sqlite3_memory_used() == baseline);
  printf("{\"allocations\":129,\"releases\":129,\"live\":0,\"owner\":\"caller\",\"sqlite_"
         "baseline\":%lld}\n",
         (long long)baseline);
  return 0;
}
