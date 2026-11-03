#ifndef RINZU_RESOURCE_REPORT_H
#define RINZU_RESOURCE_REPORT_H
#include "rinzu/resource_counters.h"
#include "rinzu/callback_consumption.h"
#include "rinzu/native_userdata.h"
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <sys/stat.h>
#include <time.h>
#ifdef __APPLE__
#include <libproc.h>
#include <sys/resource.h>
#endif
#ifndef RINZU_RESOURCE_SOURCES
#define RINZU_RESOURCE_SOURCES "{}"
#endif
#ifndef RINZU_RESOURCE_PRODUCER
#define RINZU_RESOURCE_PRODUCER "c-aot"
#endif

/* One sampler per runtime image, joined before image/code retirement. The
 * sampler reads copied counter snapshots, never Domain or queue pointers. */
static FILE *rinzu_resource_file;
static pthread_mutex_t rinzu_resource_report_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t rinzu_resource_report_condition = PTHREAD_COND_INITIALIZER;
static pthread_t rinzu_resource_sampler;
static int rinzu_resource_sampling, rinzu_resource_stopped, rinzu_resource_error;
static uint64_t rinzu_resource_sequence, rinzu_resource_interval;
static struct timespec rinzu_resource_started;
static int rinzu_resource_first;
static inline void rinzu_resource_json_string(const char *text) {
  fputc('"', rinzu_resource_file);
  for (const unsigned char *byte = (const unsigned char *)text; *byte; ++byte) {
    if (*byte == '"' || *byte == '\\') {
      fputc('\\', rinzu_resource_file);
      fputc(*byte, rinzu_resource_file);
    } else if (*byte < 32)
      fprintf(rinzu_resource_file, "\\u%04x", *byte);
    else
      fputc(*byte, rinzu_resource_file);
  }
  fputc('"', rinzu_resource_file);
}
static inline void rinzu_resource_amount(uint64_t value, int known) {
  if (known)
    fprintf(rinzu_resource_file, "%" PRIu64, value);
  else
    fputs("null", rinzu_resource_file);
}
/* mask: live/allocated/released/peak, 1/2/4/8. */
static inline void rinzu_resource_row(const char *id, const char *scope, const char *unit,
                                      const char *status, uint64_t live, uint64_t allocated,
                                      uint64_t released, uint64_t peak, unsigned mask,
                                      const char *reason, const char *subset,
                                      const rinzu_resource_site_t *site) {
  if (!rinzu_resource_first)
    fputc(',', rinzu_resource_file);
  rinzu_resource_first = 0;
  fputs("{\"id\":", rinzu_resource_file);
  rinzu_resource_json_string(id);
  fputs(",\"scope\":", rinzu_resource_file);
  rinzu_resource_json_string(scope);
  fputs(",\"unit\":", rinzu_resource_file);
  rinzu_resource_json_string(unit);
  fputs(",\"status\":", rinzu_resource_file);
  rinzu_resource_json_string(status);
  fputs(",\"live\":", rinzu_resource_file);
  rinzu_resource_amount(live, mask & 1);
  fputs(",\"allocated\":", rinzu_resource_file);
  rinzu_resource_amount(allocated, mask & 2);
  fputs(",\"released\":", rinzu_resource_file);
  rinzu_resource_amount(released, mask & 4);
  fputs(",\"peak\":", rinzu_resource_file);
  rinzu_resource_amount(peak, mask & 8);
  fputs(",\"retention_reason\":", rinzu_resource_file);
  rinzu_resource_json_string(reason);
  fputs(",\"subset_of\":", rinzu_resource_file);
  if (subset)
    rinzu_resource_json_string(subset);
  else
    fputs("null", rinzu_resource_file);
  fputs(",\"attribution\":", rinzu_resource_file);
  if (site) {
    fprintf(rinzu_resource_file, "{\"source_file\":%" PRIu64 ",\"symbol\":", site->file);
    rinzu_resource_json_string(site->symbol);
    fprintf(rinzu_resource_file,
            ",\"start_byte\":%" PRIu64 ",\"end_byte\":%" PRIu64 ",\"precision\":\"site\"}",
            site->start, site->end);
  } else {
    fprintf(rinzu_resource_file,
            "{\"source_file\":null,\"symbol\":null,\"start_byte\":null,\"end_byte\":null,"
            "\"precision\":\"%s\"}",
            !strcmp(scope, "run") && !strcmp(status, "measured") ? "run" : "unknown");
  }
  fputc('}', rinzu_resource_file);
}
static inline void rinzu_resource_retained(const char *id, const char *unit, uint64_t amount,
                                           const char *subset) {
  rinzu_resource_row(
      id, "runtime-image", unit, "measured", amount, amount, 0, amount, 15,
      "Process/image identity or immutable callback storage retained until process exit", subset,
      NULL);
}
static inline void rinzu_resource_unknown(const char *id, const char *scope, const char *unit,
                                          const char *reason) {
  rinzu_resource_row(id, scope, unit, "unknown", 0, 0, 0, 0, 0, reason, NULL, NULL);
}
static inline int rinzu_resource_write_snapshot(const char *phase) {
  if (rinzu_resource_error)
    return 1;
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now))
    return rinzu_resource_error = 1;
  uint64_t seconds = (uint64_t)(now.tv_sec - rinzu_resource_started.tv_sec);
  int64_t nanos = now.tv_nsec - rinzu_resource_started.tv_nsec;
  if (nanos < 0) {
    if (!seconds)
      _Exit(86);
    --seconds;
    nanos += INT64_C(1000000000);
  }
  if (seconds > (UINT64_MAX - (uint64_t)nanos) / UINT64_C(1000000000))
    _Exit(71);
  uint64_t elapsed = seconds * UINT64_C(1000000000) + (uint64_t)nanos;
  fprintf(rinzu_resource_file,
          "{\"schema\":\"rinzu.resource-report\",\"version\":1,\"producer\":\"%s\",\"process_id\":%"
          "ld,\"sequence\":%" PRIu64 ",\"phase\":\"%s\",\"elapsed_ns\":%" PRIu64
          ",\"consistency\":\"subsystem-consistent; "
          "cross-subsystem-best-effort\",\"sources\":%s,\"categories\":[",
          RINZU_RESOURCE_PRODUCER, (long)getpid(), rinzu_resource_sequence, phase, elapsed,
          RINZU_RESOURCE_SOURCES);
  if (rinzu_resource_sequence == UINT64_MAX)
    _Exit(71);
  ++rinzu_resource_sequence;
  rinzu_resource_first = 1;
  rinzu_resource_counter_t counters[RINZU_RESOURCE_COUNT];
  rinzu_resource_lock();
  memcpy(counters, rinzu_resource_counters, sizeof counters);
  rinzu_resource_unlock();
  static const char *ids[RINZU_RESOURCE_COUNT] = {
      "managed",        "managed_objects", "native_input",
      "task",           "future",          "pending_operation",
      "queue",          "transport",       "transport_in_flight",
      "native_calls",   "native_copy",     "registration",
      "runtime_control"};
  for (unsigned index = 0; index < RINZU_RESOURCE_COUNT; ++index) {
    const char *unit = index == RINZU_RESOURCE_MANAGED || index == RINZU_RESOURCE_NATIVE_INPUT ||
                               index == RINZU_RESOURCE_TRANSPORT ||
                               index == RINZU_RESOURCE_NATIVE_COPY
                           ? "bytes"
                           : "count";
    if (index == RINZU_RESOURCE_QUEUE || index == RINZU_RESOURCE_REGISTRATION ||
        index == RINZU_RESOURCE_RUNTIME_CONTROL) {
      rinzu_resource_unknown(ids[index], "runtime-image", unit,
                             "No image-wide observation hook for this C subsystem");
      continue;
    }
    rinzu_resource_counter_t counter = counters[index];
    rinzu_resource_row(
        ids[index], "runtime-image", unit, "measured", counter.live, counter.allocated,
        counter.released, counter.peak, index == RINZU_RESOURCE_NATIVE_COPY ? 2 : 15,
        index == RINZU_RESOURCE_MANAGED
            ? "Owned C value metadata/payload/slots/lookup index; excludes allocator overhead "
              "and GC scratch; moving GC is not new allocation"
            : "Observed image work/credit or conversion until physical settlement",
        NULL, NULL);
  }
  rinzu_return_snapshot_t snapshot = rinzu_return_snapshot();
  rinzu_callback_return_stats_t returns = snapshot.process;
  rinzu_return_scope_stats_t scopes = snapshot.scopes;
  rinzu_resource_row("callback_scoped", "runtime-image", "bytes", "measured", scopes.live_bytes,
                     scopes.allocated_bytes, scopes.released_bytes, scopes.peak_bytes, 15,
                     "Actual return owner until closed proof + physical readers/receipts settle; "
                     "promotion is transfer, not free",
                     NULL, NULL);
  rinzu_resource_row("callback_promoted", "runtime-image", "bytes", "measured", 0,
                     scopes.promoted_bytes, 0, 0, 2,
                     "Cumulative missing-proof/receipt transfer into process arena; included in "
                     "normal/fallback retention; no extra copy",
                     NULL, NULL);
  size_t process_copy = snapshot.process_copied_bytes;
  if (process_copy > SIZE_MAX - scopes.copied_bytes)
    _Exit(71);
  rinzu_resource_row("callback_return_copy", "runtime-image", "bytes", "measured", 0,
                     process_copy + scopes.copied_bytes, 0, 0, 2,
                     "Cumulative explicit memcpy prefix of process/scoped return factories; "
                     "excludes initialized padding and GC copying",
                     NULL, NULL);
  rinzu_resource_row("callback_scoped_metadata", "runtime-image", "bytes", "measured",
                     scopes.metadata_bytes, 0, 0, 0, 1,
                     "Current scope + node metadata until consumption/promotion; promoted nodes "
                     "are in callback_return_metadata",
                     NULL, NULL);
  rinzu_resource_row(
      "callback_scoped_owners", "runtime-image", "count", "measured", scopes.active_scopes, 0, 0, 0,
      1, "Actual prepared/exposed owners until physical consumption or conservative promotion",
      NULL, NULL);
  rinzu_resource_retained("callback_normal", "bytes", returns.normal_bytes, NULL);
  rinzu_resource_retained("callback_fallback", "bytes", returns.fallback_bytes, NULL);
  rinzu_resource_retained("callback_return_metadata", "bytes", returns.metadata_bytes, NULL);
#ifdef RINZU_AOT_HAS_SOURCE_NATIVE
  rinzu_callback_stats_t pool = rinzu_callback_entry_stats();
  rinzu_resource_row(
      "callback_control", "runtime-image", "bytes", "measured", pool.control_bytes,
      pool.control_bytes + pool.released_control_bytes, pool.released_control_bytes, 0, 7,
      "Full controls plus permanent RW reservation; proof retires full controls", NULL, NULL);
  rinzu_resource_retained("callback_tombstone", "bytes", pool.tombstone_bytes, "callback_control");
  rinzu_resource_retained("callback_rx", "bytes", pool.virtual_bytes, NULL);
  rinzu_resource_retained("callback_block_metadata", "bytes", pool.block_metadata_bytes, NULL);
  rinzu_resource_retained("callback_entries", "count", pool.entries, NULL);
  rinzu_resource_retained("callback_retired_entries", "count", pool.retired_entries,
                          "callback_entries");
  rinzu_resource_retained("callback_retired_slots", "bytes", pool.retired_slot_bytes,
                          "callback_control");
  rinzu_resource_row("callback_in_flight", "runtime-image", "count", "measured", pool.in_flight, 0,
                     0, 0, 1, "Admitted invocations until physical return", NULL, NULL);
  if (!strcmp(phase, "end")) {
    rinzu_resource_row("native_calls_run", "run", "count", "measured",
                       atomic_load(&rinzu_native_evidence.live_calls),
                       atomic_load(&rinzu_native_evidence.prepared),
                       atomic_load(&rinzu_native_evidence.released) +
                           atomic_load(&rinzu_native_evidence.abandoned),
                       0, 7, "Completed native call ledger; subset of image observation",
                       "native_calls", NULL);
    rinzu_resource_row("native_copy_run", "run", "bytes", "measured", 0,
                       atomic_load(&rinzu_native_evidence.copied_bytes), 0, 0, 2,
                       "Completed source native conversion evidence; SDK internal copies excluded",
                       "native_copy", NULL);
  }
#else
  static const char *pool_ids[] = {"callback_control", "callback_tombstone",
                                   "callback_rx",      "callback_block_metadata",
                                   "callback_entries", "callback_in_flight"};
  for (unsigned index = 0; index < sizeof pool_ids / sizeof *pool_ids; ++index)
    rinzu_resource_row(pool_ids[index], "runtime-image", index < 4 ? "bytes" : "count",
                       "unsupported", 0, 0, 0, 0, 0,
                       "Callback pool not linked into this runtime image", NULL, NULL);
  rinzu_resource_row("callback_retired_entries", "runtime-image", "count", "unsupported", 0, 0, 0,
                     0, 0, "Callback pool not linked into this runtime image", "callback_entries",
                     NULL);
  rinzu_resource_row("callback_retired_slots", "runtime-image", "bytes", "unsupported", 0, 0, 0, 0,
                     0, "Callback pool not linked into this runtime image", "callback_control",
                     NULL);
#endif
  rinzu_userdata_process_stats_t userdata = rinzu_userdata_process_stats();
  rinzu_resource_retained("userdata_identity", "bytes", userdata.token_bytes, NULL);
  rinzu_resource_retained("userdata_closed", "bytes", 0, "userdata_identity");
  rinzu_resource_retained("userdata_tokens", "count", userdata.tokens, NULL);
  rinzu_resource_retained("userdata_closed_tokens", "count", userdata.closed, "userdata_tokens");
  rinzu_resource_lock();
  for (const rinzu_resource_site_t *site = rinzu_resource_sites; site; site = site->next)
    rinzu_resource_row(site->fallback ? "callback_fallback_site" : "callback_normal_site",
                       "runtime-image", "bytes", "measured", site->bytes, site->bytes, 0,
                       site->bytes, 15, "Known callback site; subset of retained payload total",
                       site->fallback ? "callback_fallback" : "callback_normal", site);
  rinzu_resource_unlock();
  int foreign_known = 0;
  if (!strcmp(phase, "end")) {
#ifndef RINZU_AOT_HAS_SUPPLANT
    foreign_known = 1; /* This image did not create a foreign queue. */
#else
    foreign_known = rinzu_resource_foreign_known;
#endif
  }
  if (foreign_known) {
    rinzu_resource_row("foreign_queue", "run", "bytes", "measured", rinzu_resource_foreign_bytes, 0,
                       0, 0, 1, "Copied quiescent run-end queue evidence; peak not observed", NULL,
                       NULL);
    rinzu_resource_row("foreign_records", "run", "count", "measured", rinzu_resource_foreign_live,
                       rinzu_resource_foreign_created, rinzu_resource_foreign_retired, 0, 7,
                       "Copied quiescent run-end records; peak not observed", NULL, NULL);
  } else {
    rinzu_resource_unknown("foreign_queue", "run", "bytes",
                           "No safe image-wide queue registry; end evidence is separate");
    rinzu_resource_unknown("foreign_records", "run", "count",
                           "No safe image-wide queue registry; end evidence is separate");
  }
  rinzu_resource_unknown("jit_rx", "run", "bytes",
                         "Code size and mapping count are not mapped bytes");
  rinzu_resource_unknown("sdk_memory", "sdk", "bytes",
                         "SDK has not reported allocator-owned memory");
  uint64_t rss = 0, virtual_bytes = 0, footprint = 0;
  int task_known = 0, footprint_known = 0;
#ifdef __APPLE__
  struct proc_taskinfo task = {0};
  task_known = proc_pidinfo(getpid(), PROC_PIDTASKINFO, 0, &task, sizeof task) == sizeof task;
  if (task_known) {
    rss = task.pti_resident_size;
    virtual_bytes = task.pti_virtual_size;
  }
  struct rusage_info_v0 usage = {0};
  footprint_known = proc_pid_rusage(getpid(), RUSAGE_INFO_V0, (rusage_info_t *)&usage) == 0;
  if (footprint_known)
    footprint = usage.ri_phys_footprint;
#endif
  const char *os_ids[] = {"rss", "virtual", "footprint"};
  uint64_t os_values[] = {rss, virtual_bytes, footprint};
  int os_known[] = {task_known, task_known, footprint_known};
  for (unsigned index = 0; index < 3; ++index)
    rinzu_resource_row(
        os_ids[index], "process-os", "bytes", os_known[index] ? "measured" : "unknown",
        os_values[index], 0, 0, 0, os_known[index] ? 1 : 0,
        "Independent OS value includes sampler, SDK and allocator; not runtime category sum", NULL,
        NULL);
  fputs("]}\n", rinzu_resource_file);
  if (fflush(rinzu_resource_file) || ferror(rinzu_resource_file))
    return rinzu_resource_error = 1;
  return 0;
}
static inline void *rinzu_resource_sample_loop(void *unused) {
  (void)unused;
  if (pthread_mutex_lock(&rinzu_resource_report_mutex))
    _Exit(86);
  while (!rinzu_resource_stopped && !rinzu_resource_error) {
    struct timespec until;
    if (clock_gettime(CLOCK_REALTIME, &until)) {
      rinzu_resource_error = 1;
      break;
    }
    uint64_t seconds = rinzu_resource_interval / 1000;
    if (seconds > INT64_MAX - (uint64_t)until.tv_sec - 1) {
      rinzu_resource_error = 1;
      break;
    }
    until.tv_sec += (time_t)seconds;
    until.tv_nsec += (long)(rinzu_resource_interval % 1000) * 1000000;
    if (until.tv_nsec >= 1000000000) {
      ++until.tv_sec;
      until.tv_nsec -= 1000000000;
    }
    int status = 0;
    while (!rinzu_resource_stopped && !status)
      status = pthread_cond_timedwait(&rinzu_resource_report_condition,
                                      &rinzu_resource_report_mutex, &until);
    if (rinzu_resource_stopped)
      break;
    if (status != ETIMEDOUT) {
      rinzu_resource_error = 1;
      break;
    }
    rinzu_resource_write_snapshot("sample");
  }
  if (pthread_mutex_unlock(&rinzu_resource_report_mutex))
    _Exit(86);
  return NULL;
}
static inline int rinzu_resource_report_start(const char *path, uint64_t interval) {
  if (!path)
    return 0;
  int descriptor = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
  if (descriptor < 0) {
    fprintf(stderr, "rinzu: open resource report: %s\n", strerror(errno));
    return 1;
  }
  rinzu_resource_file = fdopen(descriptor, "w");
  if (!rinzu_resource_file) {
    close(descriptor);
    fprintf(stderr, "rinzu: open resource report stream failed\n");
    return 1;
  }
  rinzu_resource_interval = interval;
  atomic_store_explicit(&rinzu_resource_enabled, 1, memory_order_relaxed);
  if (clock_gettime(CLOCK_MONOTONIC, &rinzu_resource_started) ||
      rinzu_resource_write_snapshot("start")) {
    fclose(rinzu_resource_file);
    rinzu_resource_file = NULL;
    fprintf(stderr, "rinzu: write resource report failed\n");
    return 1;
  }
  if (interval) {
    if (pthread_create(&rinzu_resource_sampler, NULL, rinzu_resource_sample_loop, NULL)) {
      fclose(rinzu_resource_file);
      rinzu_resource_file = NULL;
      fprintf(stderr, "rinzu: start resource sampler failed\n");
      return 1;
    }
    rinzu_resource_sampling = 1;
  }
  return 0;
}
static inline int rinzu_resource_report_from_env(void) {
  const char *path = getenv("RINZU_RESOURCE_REPORT");
  if (!path)
    return 0;
  const char *text = getenv("RINZU_RESOURCE_INTERVAL_MS");
  uint64_t interval = 100;
  if (text) {
    char *end = NULL;
    errno = 0;
    interval = strtoull(text, &end, 10);
    if (!*text || *text == '-' || *text == '+' || errno || !end || *end) {
      fprintf(stderr, "rinzu: invalid RINZU_RESOURCE_INTERVAL_MS\n");
      return 1;
    }
    for (const char *byte = text; *byte; ++byte)
      if (*byte < '0' || *byte > '9') {
        fprintf(stderr, "rinzu: invalid RINZU_RESOURCE_INTERVAL_MS\n");
        return 1;
      }
  }
  return rinzu_resource_report_start(path, interval);
}
static inline int rinzu_resource_report_finish(void) {
  if (!rinzu_resource_file)
    return 0;
  if (rinzu_resource_sampling) {
    if (pthread_mutex_lock(&rinzu_resource_report_mutex))
      _Exit(86);
    rinzu_resource_stopped = 1;
    pthread_cond_signal(&rinzu_resource_report_condition);
    if (pthread_mutex_unlock(&rinzu_resource_report_mutex) ||
        pthread_join(rinzu_resource_sampler, NULL))
      _Exit(86);
  }
  int error = rinzu_resource_write_snapshot("end");
  if (fclose(rinzu_resource_file))
    error = 1;
  rinzu_resource_file = NULL;
  atomic_store_explicit(&rinzu_resource_enabled, 0, memory_order_relaxed);
  if (error)
    fprintf(stderr, "rinzu: write resource report failed\n");
  return error;
}
#endif
