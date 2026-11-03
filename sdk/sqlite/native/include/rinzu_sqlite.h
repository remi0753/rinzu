#ifndef RINZU_SQLITE_H
#define RINZU_SQLITE_H
#include <stdint.h>
#include <stdbool.h>

/* These are checked control addresses, never sqlite3/sqlite3_stmt pointers. */
enum RinzuSqliteError {
  RINZU_SQLITE_KIND = -7001,
  RINZU_SQLITE_OWNER = -7002,
  RINZU_SQLITE_CLOSED = -7003,
  RINZU_SQLITE_IN_FLIGHT = -7004,
  RINZU_SQLITE_REENTRY = -7005,
  RINZU_SQLITE_ROW = -7006,
  RINZU_SQLITE_COLUMN = -7007,
  RINZU_SQLITE_TYPE = -7008,
  RINZU_SQLITE_SINGLE_STATEMENT = -7009,
  RINZU_SQLITE_BOUND = -7010,
  RINZU_SQLITE_NO_RESULT = -7011,
  RINZU_SQLITE_DEADLINE = -7012
};
struct ValueResult {
  int32_t status;
  int64_t value;
};
void *rinzu_sqlite_open(const char *path);
int32_t rinzu_sqlite_connection_status(void *connection);
int32_t rinzu_sqlite_connection_state(void *connection);
int32_t rinzu_sqlite_execute(void *connection, const char *sql);
void rinzu_sqlite_query_int(void *connection, const char *sql, struct ValueResult *output);
void *rinzu_sqlite_prepare(void *connection, const char *sql);
int32_t rinzu_sqlite_statement_status(void *statement);
int32_t rinzu_sqlite_statement_state(void *statement);
int32_t rinzu_sqlite_step(void *statement);
void rinzu_sqlite_column_int(void *statement, int64_t index, struct ValueResult *output);
int32_t rinzu_sqlite_finalize(void *statement);
int32_t rinzu_sqlite_close(void *connection);
int32_t rinzu_sqlite_close_owner_connections(void);
int32_t rinzu_sqlite_observe(void *connection, const char *sql, void *context, int32_t (*handler)(void *));
int64_t rinzu_sqlite_retained_control_bytes(void);
int32_t rinzu_sqlite_owned_ready(void *connection);
int32_t rinzu_sqlite_owned_begin(void *connection, const char *sql, bool text);
int64_t rinzu_sqlite_owned_length(void *connection);
const uint8_t *rinzu_sqlite_owned_bytes(void *connection);
int32_t rinzu_sqlite_owned_release(void *connection);
int64_t rinzu_sqlite_owned_allocations(void);
int64_t rinzu_sqlite_owned_releases(void);
int64_t rinzu_sqlite_owned_live(void);
int32_t rinzu_sqlite_deadline(void *connection, int64_t milliseconds);
int32_t rinzu_sqlite_transaction_state(void *connection);
int32_t rinzu_sqlite_owned_rows_begin(void *connection, const char *sql, int64_t maximum_rows);
int64_t rinzu_sqlite_owned_rows(void *connection);
int64_t rinzu_sqlite_owned_row_length(void *connection, int64_t row);
int64_t rinzu_sqlite_live_connections(void);
int64_t rinzu_sqlite_live_statements(void);
int64_t rinzu_sqlite_allocator_current(void);
int64_t rinzu_sqlite_allocator_peak(void);
int32_t rinzu_sqlite_enable_allocator_stats(void);
int64_t rinzu_sqlite_monotonic_nanos(void);
const char *rinzu_sqlite_library_version(void);
const char *rinzu_sqlite_header_version(void);
const char *rinzu_sqlite_source_id(void);
#endif
