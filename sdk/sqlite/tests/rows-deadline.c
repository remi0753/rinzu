#include "rinzu_sqlite.h"
#include <assert.h>
#include <pthread.h>
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static const char *slow = "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE "
                          "x<1000000000) SELECT sum(x) FROM n";
static struct ValueResult query(void *c, const char *sql) {
  struct ValueResult r;
  rinzu_sqlite_query_int(c, sql, &r);
  return r;
}
static void *foreign(void *c) {
  for (int n = 0; n < 10000; n++) {
    assert(rinzu_sqlite_deadline(c, 1) == RINZU_SQLITE_OWNER);
    assert(rinzu_sqlite_transaction_state(c) == RINZU_SQLITE_OWNER);
    assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00'", 1) == RINZU_SQLITE_OWNER);
    assert(rinzu_sqlite_owned_release(c) == RINZU_SQLITE_OWNER);
  }
  return NULL;
}
static int32_t reentry(void *c) {
  assert(rinzu_sqlite_deadline(c, 1) == RINZU_SQLITE_REENTRY);
  assert(rinzu_sqlite_transaction_state(c) == RINZU_SQLITE_REENTRY);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00'", 1) == RINZU_SQLITE_REENTRY);
  return 0;
}
int main(void) {
  assert(rinzu_sqlite_enable_allocator_stats() == 0);
  int64_t baseline = rinzu_sqlite_allocator_current();
  void *c = rinzu_sqlite_open(":memory:");
  assert(rinzu_sqlite_connection_status(c) == 0);
  assert(rinzu_sqlite_live_connections() == 1 && rinzu_sqlite_live_statements() == 0);
  assert(rinzu_sqlite_deadline(c, -1) == RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_deadline(c, 60001) == RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_deadline(c, 10) == 0);
  assert(rinzu_sqlite_observe(c, "SELECT 1", c, reentry) == 0);
  assert(rinzu_sqlite_execute(c, "BEGIN") == 0);
  assert(rinzu_sqlite_transaction_state(c) == 1);
  int64_t start = rinzu_sqlite_monotonic_nanos();
  assert(query(c, slow).status == RINZU_SQLITE_DEADLINE);
  int64_t elapsed = rinzu_sqlite_monotonic_nanos() - start;
  assert(elapsed >= 10000000 && elapsed < 5000000000LL);
  if (rinzu_sqlite_transaction_state(c) == 1)
    assert(rinzu_sqlite_execute(c, "ROLLBACK") == 0);
  assert(query(c, "SELECT 42").value == 42);
  assert(rinzu_sqlite_execute(c, "CREATE TABLE interrupted(x);BEGIN") == 0);
  assert(rinzu_sqlite_execute(c, "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n "
                                 "WHERE x<1000000000) INSERT INTO interrupted SELECT x FROM n") ==
         RINZU_SQLITE_DEADLINE);
  assert(rinzu_sqlite_transaction_state(c) == 0);
  assert(query(c, "SELECT count(*) FROM interrupted").value == 0);
  assert(rinzu_sqlite_owned_rows_begin(c, slow, 32) == RINZU_SQLITE_DEADLINE);
  assert(rinzu_sqlite_owned_live() == 0);
  assert(rinzu_sqlite_deadline(c, 0) == 0);
  int64_t controls = rinzu_sqlite_retained_control_bytes();
  for (int n = 0; n < 1000; n++) {
    assert(rinzu_sqlite_owned_rows_begin(
               c, "SELECT X'0001ff' UNION ALL SELECT zeroblob(0) UNION ALL SELECT X'80'", 3) == 0);
    assert(rinzu_sqlite_owned_rows(c) == 3 && rinzu_sqlite_owned_length(c) == 4);
    assert(rinzu_sqlite_owned_row_length(c, 0) == 3 && rinzu_sqlite_owned_row_length(c, 1) == 0 &&
           rinzu_sqlite_owned_row_length(c, 2) == 1);
    unsigned char copy[4];
    memcpy(copy, rinzu_sqlite_owned_bytes(c), 4);
    assert(rinzu_sqlite_close(c) == RINZU_SQLITE_IN_FLIGHT);
    assert(rinzu_sqlite_deadline(c, 1) == RINZU_SQLITE_REENTRY);
    assert(rinzu_sqlite_owned_release(c) == 0);
    assert(memcmp(copy, "\0\1\xff\x80", 4) == 0);
    assert(rinzu_sqlite_owned_live() == 0 && rinzu_sqlite_live_statements() == 0);
    assert(rinzu_sqlite_retained_control_bytes() == controls);
  }
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT zeroblob(0) WHERE 0", 1) == 0);
  assert(rinzu_sqlite_owned_rows(c) == 0 && rinzu_sqlite_owned_length(c) == 0);
  assert(rinzu_sqlite_owned_release(c) == 0);
  assert(rinzu_sqlite_owned_rows_begin(c,
                                       "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 "
                                       "FROM n WHERE x<255) SELECT zeroblob(256) FROM n",
                                       256) == 0);
  assert(rinzu_sqlite_owned_rows(c) == 256 && rinzu_sqlite_owned_length(c) == 65536);
  assert(rinzu_sqlite_owned_release(c) == 0);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT zeroblob(32769) UNION ALL SELECT zeroblob(32768)",
                                       2) == RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00' UNION ALL SELECT X'01'", 1) ==
         RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00',X'01'", 1) == RINZU_SQLITE_COLUMN);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT NULL", 1) == RINZU_SQLITE_TYPE);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT 'text'", 1) == RINZU_SQLITE_TYPE);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT 1", 1) == RINZU_SQLITE_TYPE);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00';SELECT X'01'", 1) ==
         RINZU_SQLITE_SINGLE_STATEMENT);
  assert(rinzu_sqlite_owned_rows_begin(c, "SELECT X'00'", 257) == RINZU_SQLITE_BOUND);
  assert(rinzu_sqlite_owned_release(c) == RINZU_SQLITE_NO_RESULT);
  pthread_t t;
  assert(pthread_create(&t, NULL, foreign, c) == 0);
  assert(rinzu_sqlite_close(c) == 0);
  assert(pthread_join(t, NULL) == 0);
  assert(rinzu_sqlite_deadline(c, 1) == RINZU_SQLITE_CLOSED);
  assert(rinzu_sqlite_allocator_current() == baseline);
  char path[] = "/tmp/rinzu-sqlite-busy-XXXXXX";
  int fd = mkstemp(path);
  assert(fd >= 0);
  assert(close(fd) == 0);
  sqlite3 *blocker = NULL;
  assert(sqlite3_open(path, &blocker) == 0);
  assert(sqlite3_exec(blocker, "CREATE TABLE t(x);BEGIN IMMEDIATE", NULL, NULL, NULL) == 0);
  c = rinzu_sqlite_open(path);
  assert(rinzu_sqlite_connection_status(c) == 0);
  assert(rinzu_sqlite_deadline(c, 20) == 0);
  start = rinzu_sqlite_monotonic_nanos();
  assert(rinzu_sqlite_execute(c, "INSERT INTO t VALUES(1)") == RINZU_SQLITE_DEADLINE);
  int64_t busy_elapsed = rinzu_sqlite_monotonic_nanos() - start;
  assert(busy_elapsed >= 20000000 && busy_elapsed < 5000000000LL);
  assert(sqlite3_exec(blocker, "ROLLBACK", NULL, NULL, NULL) == 0);
  assert(sqlite3_close(blocker) == 0);
  assert(rinzu_sqlite_execute(c, "INSERT INTO t VALUES(1)") == 0);
  assert(rinzu_sqlite_close(c) == 0);
  assert(unlink(path) == 0);
  assert(rinzu_sqlite_live_connections() == 0 && rinzu_sqlite_live_statements() == 0);
  assert(rinzu_sqlite_owned_live() == 0 &&
         rinzu_sqlite_owned_allocations() == rinzu_sqlite_owned_releases());
  assert(rinzu_sqlite_allocator_current() == baseline);
  printf("ROWS_DEADLINE_PASS progress_ns=%lld busy_ns=%lld controls=%lld peak=%lld library=%s "
         "header=%s source=%s\n",
         (long long)elapsed, (long long)busy_elapsed,
         (long long)rinzu_sqlite_retained_control_bytes(), (long long)rinzu_sqlite_allocator_peak(),
         rinzu_sqlite_library_version(), rinzu_sqlite_header_version(), rinzu_sqlite_source_id());
}
