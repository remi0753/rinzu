#include "rinzu_sqlite.h"
#include <assert.h>
#include <pthread.h>
#include <sqlite3.h>
#include <stdio.h>
static int32_t inside(void *c) {
  (void)c;
  assert(rinzu_sqlite_close_owner_connections() == RINZU_SQLITE_IN_FLIGHT);
  return 0;
}
static void *foreign(void *unused) {
  (void)unused;
  assert(rinzu_sqlite_close_owner_connections() == 0);
  return NULL;
}
int main(void) {
  assert(rinzu_sqlite_enable_allocator_stats() == 0);
  void *a = rinzu_sqlite_open(":memory:"), *b = rinzu_sqlite_open(":memory:");
  assert(rinzu_sqlite_connection_status(a) == 0 && rinzu_sqlite_connection_status(b) == 0);
  pthread_t t;
  assert(pthread_create(&t, NULL, foreign, NULL) == 0);
  assert(pthread_join(t, NULL) == 0);
  assert(rinzu_sqlite_live_connections() == 2);
  assert(rinzu_sqlite_observe(a, "SELECT 1", a, inside) == 0);
  assert(rinzu_sqlite_connection_state(b) == 0);
  assert(rinzu_sqlite_owned_rows_begin(a, "SELECT X'0001ff'", 1) == 0);
  assert(rinzu_sqlite_close_owner_connections() == RINZU_SQLITE_IN_FLIGHT);
  assert(rinzu_sqlite_owned_release(a) == 0);
  (void)rinzu_sqlite_prepare(a,
                             "SELECT 1"); /* A discarded acquisition is still in the Owner scope. */
  assert(rinzu_sqlite_live_statements() == 1);
  assert(rinzu_sqlite_close_owner_connections() == 0);
  assert(rinzu_sqlite_live_connections() == 0 && rinzu_sqlite_live_statements() == 0);
  assert(rinzu_sqlite_connection_state(a) == 1 && rinzu_sqlite_connection_state(b) == 1);
  assert(rinzu_sqlite_allocator_current() == 0);
  assert(rinzu_sqlite_close_owner_connections() == 0);
  a = rinzu_sqlite_open(":memory:");
  assert(rinzu_sqlite_execute(a, "CREATE TABLE t(x UNIQUE);INSERT INTO t VALUES(1)") == 0);
  void *s = rinzu_sqlite_prepare(a, "INSERT INTO t VALUES(1)");
  assert(rinzu_sqlite_step(s) == SQLITE_CONSTRAINT);
  assert(rinzu_sqlite_close_owner_connections() == SQLITE_CONSTRAINT);
  assert(rinzu_sqlite_live_connections() == 0 && rinzu_sqlite_live_statements() == 0 &&
         rinzu_sqlite_allocator_current() == 0);
  assert(rinzu_sqlite_statement_state(s) == 1);
  puts("OWNER_DRAIN_PASS discarded_statement foreign_owner in_flight repeated_close "
       "finalize_error_physical_close");
}
