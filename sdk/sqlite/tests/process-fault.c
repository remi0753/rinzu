/* Test-only injection at the ordinary SQLite SDK's real allocation/SQL/close boundary. */
#include <sqlite3.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
static unsigned operations, queries;
static int close_failed;
static int setting(const char *name) {
  const char *v = getenv(name);
  return v ? atoi(v) : 0;
}
static int selected_exec(sqlite3 *c, const char *sql,
                         int (*callback)(void *, int, char **, char **), void *context,
                         char **error) {
  if (strstr(sql, "BEGIN;UPDATE rinzu_meta")) {
    operations++;
    if (setting("RINZU_TEST_CONNECTION_FAIL") == (int)operations)
      return SQLITE_IOERR;
  }
  return sqlite3_exec(c, sql, callback, context, error);
}
static int selected_step(sqlite3_stmt *s) {
  const char *sql = sqlite3_sql(s);
  if (sql && strstr(sql, "SELECT payload FROM rinzu_records")) {
    if (!sqlite3_stmt_busy(s))
      queries++;
    if (setting("RINZU_TEST_QUERY_FAIL") == (int)queries && sqlite3_data_count(s) > 0)
      return SQLITE_ERROR;
  }
  return sqlite3_step(s);
}
static void *selected_allocate(sqlite3_uint64 bytes) {
  return setting("RINZU_TEST_OWNED_OOM") ? NULL : sqlite3_malloc64(bytes);
}
static int selected_close(sqlite3 *c) {
  if (setting("RINZU_TEST_CLOSE_FAIL") && !close_failed++)
    return SQLITE_IOERR;
  return sqlite3_close(c);
}
#define sqlite3_exec selected_exec
#define sqlite3_step selected_step
#define sqlite3_malloc64 selected_allocate
#define sqlite3_close selected_close
#include "../native/src/sqlite_source.c"
