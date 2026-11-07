#include "rinzu_sqlite.h"
#include <assert.h>
#include <ctype.h>
#include <pthread.h>
#include <sqlite3.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum ControlKind { CONNECTION = 1, STATEMENT = 2 };
enum { OWNED_CHUNK = 65536, OWNED_ROWS = 256 };
struct Control {
  /* Written once before publication. Check these before mutable fields. */
  enum ControlKind kind;
  pthread_t owner;
  bool closed;
  int32_t acquired;
};
struct Connection {
  struct Control control;
  sqlite3 *payload;
  size_t uses, children;
  uint8_t *owned;
  size_t owned_length;
  size_t owned_rows, row_lengths[OWNED_ROWS];
  int64_t deadline_ms, expires_ns;
  bool timed_out;
  struct Connection *next_owner;
  struct Statement *statements;
};
struct Statement {
  struct Control control;
  struct Connection *parent;
  sqlite3_stmt *payload;
  bool row;
  struct Statement *next_child;
};
struct Use {
  struct Connection *connection;
};
static _Atomic int64_t retained_bytes;
static _Atomic int64_t live_connections, live_statements;
static pthread_mutex_t initialization_lock = PTHREAD_MUTEX_INITIALIZER;
static bool opened;
/* Includes acquisitions whose native return was discarded by Source cancel.
 * Controls already have process lifetime; this list does not own extra nodes. */
static _Thread_local struct Connection *owner_connections;

int64_t rinzu_sqlite_monotonic_nanos(void) {
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now) || now.tv_sec < 0 ||
      now.tv_sec > (INT64_MAX - now.tv_nsec) / INT64_C(1000000000))
    _Exit(86);
  return (int64_t)now.tv_sec * INT64_C(1000000000) + now.tv_nsec;
}
static bool expired(struct Connection *connection) {
  if (!connection->expires_ns || rinzu_sqlite_monotonic_nanos() < connection->expires_ns)
    return false;
  connection->timed_out = true;
  return true;
}
static int progress(void *raw) {
  return expired(raw);
}
static int busy(void *raw, int attempts) {
  (void)attempts;
  struct Connection *connection = raw;
  if (!connection->expires_ns || expired(connection))
    return 0;
  /* C-only callback: no connection modification, managed code or foreign hop. */
  struct timespec interval = {0, 1000000};
  (void)nanosleep(&interval, NULL);
  return !expired(connection);
}
static int32_t deadline_status(struct Connection *connection, int32_t status) {
  return connection->timed_out && (status == SQLITE_INTERRUPT || status == SQLITE_BUSY)
             ? RINZU_SQLITE_DEADLINE
             : status;
}

static void *allocate_control(size_t bytes, enum ControlKind kind) {
  struct Control *control = calloc(1, bytes);
  if (control) {
    control->kind = kind;
    control->owner = pthread_self();
    control->closed = true;
    control->acquired = SQLITE_NOMEM;
    atomic_fetch_add_explicit(&retained_bytes, (int64_t)bytes, memory_order_relaxed);
  }
  return control;
}

static int32_t owner_check(const struct Control *control, enum ControlKind kind) {
  if (!control)
    return SQLITE_NOMEM;
  if (control->kind != kind)
    return RINZU_SQLITE_KIND;
  if (!pthread_equal(control->owner, pthread_self()))
    return RINZU_SQLITE_OWNER;
  return SQLITE_OK;
}

static int32_t connection_check(const struct Connection *connection) {
  int32_t status = owner_check((const struct Control *)connection, CONNECTION);
  if (status != SQLITE_OK)
    return status;
  return connection->control.closed ? RINZU_SQLITE_CLOSED : SQLITE_OK;
}

static int32_t use_begin(struct Connection *connection, struct Use *use) {
  int32_t status = connection_check(connection);
  if (status != SQLITE_OK)
    return status;
  if (connection->uses)
    return RINZU_SQLITE_REENTRY;
  connection->uses++;
  connection->timed_out = false;
  connection->expires_ns = 0;
  if (connection->deadline_ms) {
    int64_t now = rinzu_sqlite_monotonic_nanos();
    int64_t budget = connection->deadline_ms * INT64_C(1000000);
    if (now > INT64_MAX - budget)
      _Exit(86);
    connection->expires_ns = now + budget;
  }
  use->connection = connection;
  return SQLITE_OK;
}

static void use_end(struct Use *use) {
  assert(use->connection->uses == 1);
  use->connection->uses--;
  use->connection->expires_ns = 0;
}

static int32_t statement_check(const struct Statement *statement) {
  int32_t status = owner_check((const struct Control *)statement, STATEMENT);
  if (status != SQLITE_OK)
    return status;
  if (statement->control.closed)
    return RINZU_SQLITE_CLOSED;
  return connection_check(statement->parent);
}

static int32_t prepare_one(sqlite3 *connection, const char *sql, sqlite3_stmt **statement) {
  const char *tail = NULL;
  int32_t status = sqlite3_prepare_v2(connection, sql, -1, statement, &tail);
  if (status != SQLITE_OK)
    return status;
  if (!*statement)
    return RINZU_SQLITE_SINGLE_STATEMENT;
  while (*tail && isspace((unsigned char)*tail))
    tail++;
  return *tail ? RINZU_SQLITE_SINGLE_STATEMENT : SQLITE_OK;
}

void *rinzu_sqlite_open(const char *path) {
  struct Connection *connection = allocate_control(sizeof(*connection), CONNECTION);
  if (!connection)
    return NULL;
  if (!sqlite3_threadsafe()) {
    connection->control.acquired = SQLITE_MISUSE;
    return connection;
  }
  int flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
  if (pthread_mutex_lock(&initialization_lock))
    _Exit(86);
  opened = true;
  int32_t status = sqlite3_open_v2(path, &connection->payload, flags, NULL);
  if (pthread_mutex_unlock(&initialization_lock))
    _Exit(86);
  connection->control.acquired = status;
  if (status == SQLITE_OK) {
    connection->control.closed = false;
    connection->next_owner = owner_connections;
    owner_connections = connection;
    atomic_fetch_add_explicit(&live_connections, 1, memory_order_relaxed);
  } else if (connection->payload) {
    (void)sqlite3_close(connection->payload);
    connection->payload = NULL;
  }
  return connection;
}

int32_t rinzu_sqlite_connection_status(void *value) {
  struct Connection *connection = value;
  int32_t status = owner_check(value, CONNECTION);
  return status == SQLITE_OK ? connection->control.acquired : status;
}

int32_t rinzu_sqlite_connection_state(void *value) {
  struct Connection *connection = value;
  int32_t status = owner_check(value, CONNECTION);
  return status == SQLITE_OK ? (int32_t)connection->control.closed : status;
}

int32_t rinzu_sqlite_execute(void *value, const char *sql) {
  struct Use use;
  int32_t status = use_begin(value, &use);
  if (status != SQLITE_OK)
    return status;
  status = sqlite3_exec(use.connection->payload, sql, NULL, NULL, NULL);
  status = deadline_status(use.connection, status);
  use_end(&use);
  return status;
}

void rinzu_sqlite_query_int(void *value, const char *sql, struct ValueResult *output) {
  struct ValueResult result = {SQLITE_OK, 0};
  struct Use use;
  result.status = use_begin(value, &use);
  if (result.status != SQLITE_OK) {
    *output = result;
    return;
  }
  sqlite3_stmt *statement = NULL;
  result.status = prepare_one(use.connection->payload, sql, &statement);
  if (result.status == SQLITE_OK) {
    int32_t step = sqlite3_step(statement);
    if (step != SQLITE_ROW) {
      result.status = step == SQLITE_DONE ? RINZU_SQLITE_ROW : step;
    } else if (sqlite3_column_type(statement, 0) != SQLITE_INTEGER) {
      result.status = RINZU_SQLITE_TYPE;
    } else {
      result.value = sqlite3_column_int64(statement, 0);
    }
  }
  if (statement) {
    int32_t finalized = sqlite3_finalize(statement);
    if (result.status == SQLITE_OK)
      result.status = finalized;
  }
  result.status = deadline_status(use.connection, result.status);
  use_end(&use);
  *output = result;
}

void *rinzu_sqlite_prepare(void *value, const char *sql) {
  struct Statement *statement = allocate_control(sizeof(*statement), STATEMENT);
  if (!statement)
    return NULL;
  struct Use use;
  int32_t status = use_begin(value, &use);
  if (status == SQLITE_OK) {
    status = prepare_one(use.connection->payload, sql, &statement->payload);
    if (status == SQLITE_OK) {
      statement->parent = value;
      statement->control.closed = false;
      statement->parent->children++;
      statement->next_child = statement->parent->statements;
      statement->parent->statements = statement;
      atomic_fetch_add_explicit(&live_statements, 1, memory_order_relaxed);
    } else if (statement->payload) {
      (void)sqlite3_finalize(statement->payload);
      statement->payload = NULL;
    }
    status = deadline_status(use.connection, status);
    use_end(&use);
  }
  statement->control.acquired = status;
  return statement;
}

int32_t rinzu_sqlite_statement_status(void *value) {
  struct Statement *statement = value;
  int32_t status = owner_check(value, STATEMENT);
  return status == SQLITE_OK ? statement->control.acquired : status;
}

int32_t rinzu_sqlite_statement_state(void *value) {
  struct Statement *statement = value;
  int32_t status = owner_check(value, STATEMENT);
  return status == SQLITE_OK ? (int32_t)statement->control.closed : status;
}

int32_t rinzu_sqlite_step(void *value) {
  struct Statement *statement = value;
  int32_t status = statement_check(statement);
  if (status != SQLITE_OK)
    return status;
  struct Use use;
  status = use_begin(statement->parent, &use);
  if (status != SQLITE_OK)
    return status;
  status = sqlite3_step(statement->payload);
  statement->row = status == SQLITE_ROW;
  status = deadline_status(use.connection, status);
  use_end(&use);
  return status;
}

void rinzu_sqlite_column_int(void *value, int64_t index, struct ValueResult *output) {
  struct Statement *statement = value;
  struct ValueResult result = {statement_check(statement), 0};
  if (result.status != SQLITE_OK) {
    *output = result;
    return;
  }
  struct Use use;
  result.status = use_begin(statement->parent, &use);
  if (result.status != SQLITE_OK) {
    *output = result;
    return;
  }
  if (!statement->row) {
    result.status = RINZU_SQLITE_ROW;
  } else if (index < 0 || index >= sqlite3_column_count(statement->payload)) {
    result.status = RINZU_SQLITE_COLUMN;
  } else if (sqlite3_column_type(statement->payload, (int)index) != SQLITE_INTEGER) {
    result.status = RINZU_SQLITE_TYPE;
  } else {
    result.value = sqlite3_column_int64(statement->payload, (int)index);
  }
  use_end(&use);
  *output = result;
}

int32_t rinzu_sqlite_finalize(void *value) {
  struct Statement *statement = value;
  int32_t status = statement_check(statement);
  if (status != SQLITE_OK)
    return status;
  struct Use use;
  status = use_begin(statement->parent, &use);
  if (status != SQLITE_OK)
    return status;
  status = sqlite3_finalize(statement->payload);
  statement->payload = NULL;
  statement->control.closed = true;
  statement->row = false;
  assert(statement->parent->children);
  statement->parent->children--;
  atomic_fetch_sub_explicit(&live_statements, 1, memory_order_relaxed);
  use_end(&use);
  return status;
}

int32_t rinzu_sqlite_close(void *value) {
  struct Connection *connection = value;
  int32_t status = connection_check(connection);
  if (status != SQLITE_OK)
    return status;
  if (connection->uses)
    return RINZU_SQLITE_IN_FLIGHT;
  if (connection->children)
    return SQLITE_BUSY;
  status = sqlite3_close(connection->payload);
  if (status == SQLITE_OK) {
    connection->payload = NULL;
    connection->control.closed = true;
    atomic_fetch_sub_explicit(&live_connections, 1, memory_order_relaxed);
  }
  return status;
}

int32_t rinzu_sqlite_close_owner_connections(void) {
  /* Explicit Owner-scope drain, after every child/native use has returned.
   * Reject an incomplete scope before closing any of its connections. */
  for (struct Connection *c = owner_connections; c; c = c->next_owner) {
    if (!c->control.closed && c->uses)
      return RINZU_SQLITE_IN_FLIGHT;
  }
  int first_error = SQLITE_OK;
  for (struct Connection *c = owner_connections; c; c = c->next_owner) {
    if (!c->control.closed) {
      for (struct Statement *s = c->statements; s; s = s->next_child) {
        if (!s->control.closed) {
          int status = rinzu_sqlite_finalize(s);
          if (status && !first_error)
            first_error = status;
        }
      }
      int status = rinzu_sqlite_close(c);
      if (status && !first_error)
        first_error = status;
    }
  }
  return first_error;
}

struct Observer {
  void *context;
  int32_t (*handler)(void *);
};

static int observe_row(void *context, int count, char **columns, char **names) {
  (void)count;
  (void)columns;
  (void)names;
  struct Observer *observer = context;
  return observer->handler(observer->context);
}

int32_t rinzu_sqlite_observe(void *value, const char *sql, void *context,
                             int32_t (*handler)(void *)) {
  struct Use use;
  int32_t status = use_begin(value, &use);
  if (status != SQLITE_OK)
    return status;
  struct Observer observer = {context, handler};
  status = sqlite3_exec(use.connection->payload, sql, observe_row, &observer, NULL);
  status = deadline_status(use.connection, status);
  use_end(&use);
  return status;
}

int64_t rinzu_sqlite_retained_control_bytes(void) {
  return atomic_load_explicit(&retained_bytes, memory_order_relaxed);
}

/* Private owned result slot: SQLite allocator only, connection use held until release. */
static _Atomic int64_t owned_allocations, owned_releases, owned_live;
static void owned_dispose(struct Connection *connection) {
  if (!connection->owned)
    return;
  sqlite3_free(connection->owned);
  connection->owned = NULL;
  connection->owned_length = 0;
  connection->owned_rows = 0;
  atomic_fetch_add_explicit(&owned_releases, 1, memory_order_relaxed);
  atomic_fetch_sub_explicit(&owned_live, 1, memory_order_relaxed);
}
int32_t rinzu_sqlite_owned_ready(void *value) {
  struct Connection *connection = value;
  int32_t status = connection_check(connection);
  if (status != SQLITE_OK)
    return status;
  return connection->uses ? RINZU_SQLITE_REENTRY : SQLITE_OK;
}
int32_t rinzu_sqlite_owned_begin(void *value, const char *sql, bool text) {
  struct Use use;
  int32_t status = use_begin(value, &use);
  if (status != SQLITE_OK)
    return status;
  struct Connection *connection = value;
  sqlite3_stmt *statement = NULL;
  status = prepare_one(connection->payload, sql, &statement);
  if (status == SQLITE_OK) {
    int32_t step = sqlite3_step(statement);
    if (step != SQLITE_ROW)
      status = step == SQLITE_DONE ? RINZU_SQLITE_ROW : step;
    else if (sqlite3_column_type(statement, 0) != (text ? SQLITE_TEXT : SQLITE_BLOB))
      status = RINZU_SQLITE_TYPE;
    else {
      const void *borrowed = text ? (const void *)sqlite3_column_text(statement, 0)
                                  : sqlite3_column_blob(statement, 0);
      if (sqlite3_errcode(connection->payload) == SQLITE_NOMEM)
        _Exit(70);
      int length = sqlite3_column_bytes(statement, 0);
      if (sqlite3_errcode(connection->payload) == SQLITE_NOMEM)
        _Exit(70);
      if (length < 0 || length > OWNED_CHUNK)
        status = RINZU_SQLITE_BOUND;
      else {
        if (length && !borrowed)
          _Exit(70);
        connection->owned = sqlite3_malloc64(OWNED_CHUNK);
        if (!connection->owned)
          _Exit(70);
        memset(connection->owned, 0, OWNED_CHUNK);
        if (length)
          memcpy(connection->owned, borrowed, (size_t)length);
        connection->owned_length = (size_t)length;
        connection->owned_rows = 1;
        connection->row_lengths[0] = (size_t)length;
        atomic_fetch_add_explicit(&owned_allocations, 1, memory_order_relaxed);
        atomic_fetch_add_explicit(&owned_live, 1, memory_order_relaxed);
      }
    }
  }
  if (statement) {
    int32_t finalized = sqlite3_finalize(statement);
    if (status == SQLITE_OK)
      status = finalized;
  }
  if (status == SQLITE_NOMEM)
    _Exit(70);
  status = deadline_status(connection, status);
  if (status != SQLITE_OK) {
    owned_dispose(connection);
    use_end(&use);
  }
  return status;
}
static int32_t owned_check(struct Connection *connection) {
  int32_t status = connection_check(connection);
  if (status != SQLITE_OK)
    return status;
  return connection->owned && connection->uses == 1 ? SQLITE_OK : RINZU_SQLITE_NO_RESULT;
}
int64_t rinzu_sqlite_owned_length(void *value) {
  struct Connection *connection = value;
  if (owned_check(connection) != SQLITE_OK)
    _Exit(86);
  return (int64_t)connection->owned_length;
}
const uint8_t *rinzu_sqlite_owned_bytes(void *value) {
  struct Connection *connection = value;
  if (owned_check(connection) != SQLITE_OK)
    _Exit(86);
  return connection->owned;
}
int32_t rinzu_sqlite_owned_release(void *value) {
  struct Connection *connection = value;
  int32_t status = owned_check(connection);
  if (status != SQLITE_OK)
    return status;
  owned_dispose(connection);
  struct Use use = {connection};
  use_end(&use);
  return SQLITE_OK;
}
int64_t rinzu_sqlite_owned_allocations(void) {
  return atomic_load_explicit(&owned_allocations, memory_order_relaxed);
}
int64_t rinzu_sqlite_owned_releases(void) {
  return atomic_load_explicit(&owned_releases, memory_order_relaxed);
}
int64_t rinzu_sqlite_owned_live(void) {
  return atomic_load_explicit(&owned_live, memory_order_relaxed);
}

int32_t rinzu_sqlite_deadline(void *value, int64_t milliseconds) {
  struct Connection *connection = value;
  int32_t status = connection_check(connection);
  if (status)
    return status;
  if (connection->uses)
    return RINZU_SQLITE_REENTRY;
  if (milliseconds < 0 || milliseconds > 60000)
    return RINZU_SQLITE_BOUND;
  connection->deadline_ms = milliseconds;
  sqlite3_progress_handler(connection->payload, milliseconds ? 1000 : 0,
                           milliseconds ? progress : NULL, milliseconds ? connection : NULL);
  return sqlite3_busy_handler(connection->payload, milliseconds ? busy : NULL,
                              milliseconds ? connection : NULL);
}
int32_t rinzu_sqlite_transaction_state(void *value) {
  struct Connection *connection = value;
  int32_t status = connection_check(connection);
  if (status)
    return status;
  if (connection->uses)
    return RINZU_SQLITE_REENTRY;
  return !sqlite3_get_autocommit(connection->payload);
}
int32_t rinzu_sqlite_owned_rows_begin(void *value, const char *sql, int64_t maximum_rows) {
  if (maximum_rows < 1 || maximum_rows > OWNED_ROWS)
    return RINZU_SQLITE_BOUND;
  struct Use use;
  int32_t status = use_begin(value, &use);
  if (status)
    return status;
  struct Connection *connection = value;
  sqlite3_stmt *statement = NULL;
  status = prepare_one(connection->payload, sql, &statement);
  if (!status && sqlite3_column_count(statement) != 1)
    status = RINZU_SQLITE_COLUMN;
  if (!status) {
    connection->owned = sqlite3_malloc64(OWNED_CHUNK);
    if (!connection->owned)
      _Exit(70);
    memset(connection->owned, 0, OWNED_CHUNK);
    connection->owned_length = connection->owned_rows = 0;
    atomic_fetch_add_explicit(&owned_allocations, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&owned_live, 1, memory_order_relaxed);
    for (;;) {
      int step = sqlite3_step(statement);
      if (step != SQLITE_ROW) {
        status = step == SQLITE_DONE ? SQLITE_OK : step;
        break;
      }
      if (connection->owned_rows == (size_t)maximum_rows) {
        status = RINZU_SQLITE_BOUND;
        break;
      }
      if (sqlite3_column_type(statement, 0) != SQLITE_BLOB) {
        status = RINZU_SQLITE_TYPE;
        break;
      }
      const void *data = sqlite3_column_blob(statement, 0);
      int length = sqlite3_column_bytes(statement, 0);
      if (sqlite3_errcode(connection->payload) == SQLITE_NOMEM || (length && !data))
        _Exit(70);
      if (length < 0 || (size_t)length > OWNED_CHUNK - connection->owned_length) {
        status = RINZU_SQLITE_BOUND;
        break;
      }
      if (length)
        memcpy(connection->owned + connection->owned_length, data, (size_t)length);
      connection->row_lengths[connection->owned_rows++] = (size_t)length;
      connection->owned_length += (size_t)length;
    }
  }
  if (statement) {
    int finalized = sqlite3_finalize(statement);
    if (!status)
      status = finalized;
  }
  if (status == SQLITE_NOMEM)
    _Exit(70);
  status = deadline_status(connection, status);
  if (status) {
    owned_dispose(connection);
    use_end(&use);
  }
  return status;
}
int64_t rinzu_sqlite_owned_rows(void *value) {
  struct Connection *connection = value;
  if (owned_check(connection))
    _Exit(86);
  return (int64_t)connection->owned_rows;
}
int64_t rinzu_sqlite_owned_row_length(void *value, int64_t row) {
  struct Connection *connection = value;
  if (owned_check(connection) || row < 0 || (size_t)row >= connection->owned_rows)
    _Exit(86);
  return (int64_t)connection->row_lengths[row];
}
int64_t rinzu_sqlite_live_connections(void) {
  return atomic_load(&live_connections);
}
int64_t rinzu_sqlite_live_statements(void) {
  return atomic_load(&live_statements);
}
int64_t rinzu_sqlite_allocator_current(void) {
  return sqlite3_memory_used();
}
int64_t rinzu_sqlite_allocator_peak(void) {
  return sqlite3_memory_highwater(0);
}
int32_t rinzu_sqlite_enable_allocator_stats(void) {
#if defined(__APPLE__)
  if (!pthread_main_np())
    return RINZU_SQLITE_OWNER;
#endif
  /* Explicit startup configuration, before any SDK open or external SQLite use. */
  if (pthread_mutex_lock(&initialization_lock))
    _Exit(86);
  int status = opened ? SQLITE_MISUSE : sqlite3_config(SQLITE_CONFIG_MEMSTATUS, 1);
  if (pthread_mutex_unlock(&initialization_lock))
    _Exit(86);
  return status;
}
const char *rinzu_sqlite_library_version(void) {
  return sqlite3_libversion();
}
const char *rinzu_sqlite_header_version(void) {
  return SQLITE_VERSION;
}
const char *rinzu_sqlite_source_id(void) {
  return sqlite3_sourceid();
}
