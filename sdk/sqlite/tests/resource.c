#include "rinzu_sqlite.h"
#include <assert.h>
#include <pthread.h>
#include <sqlite3.h>
#include <stdint.h>
#include <stdio.h>

static struct ValueResult query(void *connection, const char *sql) {
  struct ValueResult result;
  rinzu_sqlite_query_int(connection, sql, &result);
  return result;
}
static int32_t reentry(void *connection) {
  assert(rinzu_sqlite_close(connection) == RINZU_SQLITE_IN_FLIGHT);
  assert(query(connection, "SELECT 1").status == RINZU_SQLITE_REENTRY);
  return 0;
}
static void *foreign(void *connection) {
  for (int i = 0; i < 10000; i++) {
    assert(rinzu_sqlite_close(connection) == RINZU_SQLITE_OWNER);
    assert(query(connection, "SELECT 1").status == RINZU_SQLITE_OWNER);
    assert(rinzu_sqlite_connection_state(connection) == RINZU_SQLITE_OWNER);
  }
  return NULL;
}
int main(void) {
  sqlite3_int64 baseline = sqlite3_memory_used();
  void *connection = rinzu_sqlite_open(":memory:");
  assert(rinzu_sqlite_connection_status(connection) == SQLITE_OK);
  assert(rinzu_sqlite_observe(connection, "SELECT 1 UNION ALL SELECT 2", connection, reentry) == SQLITE_OK);
  struct ValueResult value = query(connection, "SELECT 9223372036854775807");
  assert(value.status == SQLITE_OK && value.value == INT64_MAX);
  assert(query(connection, "SELECT 'borrowed text'").status == RINZU_SQLITE_TYPE);
  assert(query(connection, "SELECT 1 WHERE 0").status == RINZU_SQLITE_ROW);
  assert(query(connection, "SELECT 1; SELECT 2").status == RINZU_SQLITE_SINGLE_STATEMENT);
  void *statement = rinzu_sqlite_prepare(connection, "SELECT 9");
  assert(rinzu_sqlite_statement_status(statement) == SQLITE_OK);
  assert(rinzu_sqlite_close(connection) == SQLITE_BUSY);
  assert(rinzu_sqlite_connection_state(connection) == 0);
  rinzu_sqlite_column_int(statement, 0, &value);
  assert(value.status == RINZU_SQLITE_ROW);
  assert(rinzu_sqlite_step(statement) == SQLITE_ROW);
  rinzu_sqlite_column_int(statement, -1, &value);
  assert(value.status == RINZU_SQLITE_COLUMN);
  rinzu_sqlite_column_int(statement, 0, &value);
  assert(value.status == SQLITE_OK && value.value == 9);
  assert(rinzu_sqlite_close(statement) == RINZU_SQLITE_KIND);
  assert(rinzu_sqlite_finalize(statement) == SQLITE_OK);
  assert(rinzu_sqlite_finalize(statement) == RINZU_SQLITE_CLOSED);
  assert(rinzu_sqlite_step(statement) == RINZU_SQLITE_CLOSED);
  void *next = rinzu_sqlite_prepare(connection, "SELECT 10");
  assert(next != statement);
  assert(rinzu_sqlite_step(statement) == RINZU_SQLITE_CLOSED);
  assert(rinzu_sqlite_finalize(next) == SQLITE_OK);
  assert(rinzu_sqlite_execute(connection, "CREATE TABLE t(x UNIQUE); INSERT INTO t VALUES(1)") ==
         SQLITE_OK);
  void *bad = rinzu_sqlite_prepare(connection, "INSERT INTO t VALUES(1)");
  assert(rinzu_sqlite_step(bad) == SQLITE_CONSTRAINT);
  assert(rinzu_sqlite_finalize(bad) == SQLITE_CONSTRAINT);
  assert(rinzu_sqlite_statement_state(bad) == 1);
  assert(rinzu_sqlite_finalize(bad) == RINZU_SQLITE_CLOSED);
  pthread_t thread;
  assert(pthread_create(&thread, NULL, foreign, connection) == 0);
  assert(rinzu_sqlite_close(connection) == SQLITE_OK);
  assert(pthread_join(thread, NULL) == 0);
  assert(rinzu_sqlite_close(connection) == RINZU_SQLITE_CLOSED);
  assert(query(connection, "SELECT 1").status == RINZU_SQLITE_CLOSED);
  void *failure = rinzu_sqlite_open("/nonexistent-rinzu-parent/database");
  assert(rinzu_sqlite_connection_status(failure) != SQLITE_OK);
  assert(rinzu_sqlite_connection_state(failure) == 1);
  assert(sqlite3_memory_used() == baseline);
  printf("SQLITE_BASELINE=%lld CONTROLS=%lld\n", (long long)baseline,
         (long long)rinzu_sqlite_retained_control_bytes());
}
