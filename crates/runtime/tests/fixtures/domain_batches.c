#include "domain_trace.h"
#include <stdlib.h>
#include <unistd.h>

typedef struct {
  uint64_t owner;
  uint64_t value;
} environment_t;

typedef struct {
  rinzu_async_runtime_t *runtime;
  environment_t *environment;
  rinzu_native_completion_v2_t *storage[2];
  rinzu_async_future_t *future[2];
  rinzu_native_completion_wake_t *leader;
  pthread_t notifier;
  unsigned remaining;
} batch_t;

static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static unsigned in_context;
static unsigned may_return;

static void touch(void *raw) {
  environment_t *environment = raw;
  assert(environment->owner == domain && environment->value == 7183 + 97 * domain);
  trace("context_identity", 0, (intptr_t)raw, 0);
  trace("context_touch", producer, 0, 0);
  unsigned bit = 1u << domain;
  assert(!pthread_mutex_lock(&gate));
  in_context |= bit;
  assert(!pthread_cond_broadcast(&changed));
  while (!(may_return & bit))
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  assert(environment->owner == domain && environment->value == 7183 + 97 * domain);
}

static void *notify(void *raw) {
  const batch_t *batch = raw;
  domain = (unsigned)batch->environment->owner;
  producer = 100;
  batch->leader->notify(batch->leader);
  producer = 0;
  return NULL;
}

static void await_notifications(unsigned expected) {
  assert(!pthread_mutex_lock(&gate));
  while (in_context != expected)
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
}

static void end_notification(batch_t *batch, unsigned d) {
  unsigned bit = 1u << d;
  assert(!pthread_mutex_lock(&gate));
  assert(!(may_return & bit));
  may_return |= bit;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
  assert(!pthread_join(batch->notifier, NULL));
}

static void init(batch_t *batch, unsigned d, rinzu_native_completion_v3_t *reused) {
  domain = d;
  batch->runtime = calloc(1, sizeof(*batch->runtime));
  batch->environment = malloc(sizeof(*batch->environment));
  rinzu_native_completion_v3_t *scalar = reused ? reused : calloc(1, sizeof(*scalar));
  rinzu_native_completion_v2_t *unit = calloc(1, sizeof(*unit));
  assert(batch->runtime && batch->environment && scalar && unit);
  *batch->environment = (environment_t){d, 7183 + 97 * d};
  trace("domain_context", 0, (intptr_t)batch->environment, 0);
  rinzu_async_init(batch->runtime);
  rinzu_async_set_completion_wake(batch->runtime, touch, batch->environment);
  rinzu_native_completion_v2_init(&scalar->completion);
  scalar->completion.base.version = 3;
  scalar->completion.base.size = sizeof(*scalar);
  scalar->result = (int64_t)batch->environment->value;
  rinzu_native_completion_v2_init(unit);
  batch->storage[0] = &scalar->completion;
  batch->storage[1] = unit;
  batch->remaining = 2;
  for (unsigned g = 0; g < 2; ++g) {
    carriers[d][g] = (uintptr_t)batch->storage[g];
    trace("generation", 100 + g, g, 0);
    trace("carrier_address", 100 + g, (intptr_t)batch->storage[g], batch->storage[g]->base.version);
  }
}

static void admit(batch_t *batch, unsigned d, int controlled) {
  domain = d;
  batch->future[0] = start(batch->runtime, &batch->storage[0]->base, RINZU_SCALAR_I64);
  batch->future[1] = start(batch->runtime, &batch->storage[1]->base, RINZU_TRANSFER_UNIT);
  if (controlled) {
    rinzu_async_future_t *unit = batch->future[1];
    assert(rinzu_async_cancel(unit));
    assert(!rinzu_async_try_collect(unit));
  }
}

static rinzu_native_completion_wake_t *capture(batch_t *batch, unsigned g) {
  return capture_target(batch->storage[g], g);
}

static void merge(batch_t *batch, rinzu_native_completion_wake_t *duplicate, unsigned d) {
  domain = d;
  assert(batch->leader == duplicate && refs(duplicate) == 3);
  trace("merge_wake", 101, 100, 0);
  return_reference(1, duplicate);
  assert(refs(batch->leader) == 2);
}

static void settle(batch_t *batch, unsigned d, unsigned g, int controlled, int keep_storage) {
  domain = d;
  rinzu_async_future_t *future = batch->future[g];
  assert(rinzu_async_try_collect(future));
  assert(future->settled && (future->failure.primary != 0) == (controlled && g == 1));
  assert(future->result_slot == (g ? 0u : 7183 + 97 * d));
  rinzu_native_completion_t *carrier = &batch->storage[g]->base;
  trace("generation_status", 100 + g, atomic_load(&carrier->cancelled) != 0,
        rinzu_native_completion_is_done(carrier));
  trace("close_admission", 100 + g, 0, 0);
  if (keep_storage) {
    trace("carrier_retire", 100 + g, (intptr_t)carrier, 0);
  } else {
    free(batch->storage[g]);
    trace("carrier_free", 100 + g, 0, 0);
  }
  --batch->remaining;
}

static void dispose_batch(batch_t *batch, unsigned d) {
  domain = d;
  assert(batch->future[0]->result_slot == 7183 + 97 * d);
  for (unsigned g = 0; g < 2; ++g)
    rinzu_async_release(batch->future[g]);
  trace("runtime_refs", 0, (intptr_t)batch->leader, (int64_t)refs(batch->leader));
  trace("shutdown", 0, 0, 0);
  rinzu_async_dispose(batch->runtime);
  intptr_t address = (intptr_t)batch->environment;
  free(batch->environment);
  free(batch->runtime);
  trace("environment_identity_free", 0, address, 0);
  trace("env_free", 0, 0, 0);
  assert(refs(batch->leader) == 1);
}

static void notify_detached(batch_t *batch, unsigned d) {
  domain = d;
  producer = 100;
  batch->leader->notify(batch->leader);
  producer = 0;
}

static void finish(batch_t *batch, unsigned d, int late) {
  dispose_batch(batch, d);
  if (late)
    notify_detached(batch, d);
  return_reference(0, batch->leader);
}

static void publish(batch_t *batch, unsigned d) {
  domain = d;
  batch->leader = capture(batch, 0);
  merge(batch, capture(batch, 1), d);
}

static void publish_replacement(batch_t *batch) {
  publish(batch, 1);
  notify(batch);
  rinzu_async_native_progress(batch->runtime);
}

static void hold_replacement_notification(batch_t *batch) {
  may_return = 0;
  publish(batch, 1);
  assert(!pthread_create(&batch->notifier, NULL, notify, batch));
  await_notifications(2);
}

static uint64_t replacement_epoch(const batch_t *batch) {
  domain = 1;
  return rinzu_async_completion_epoch(batch->runtime);
}

static void overlap_notifications(batch_t *old, batch_t *new, int replacement_first) {
  hold_replacement_notification(new);
  uint64_t epoch = replacement_epoch(new);
  assert(!pthread_create(&old->notifier, NULL, notify, old));
  await_notifications(3);
  assert(replacement_epoch(new) == epoch);
  if (replacement_first) {
    end_notification(new, 1);
    end_notification(old, 0);
  } else {
    end_notification(old, 0);
    end_notification(new, 1);
  }
  domain = 1;
  rinzu_async_native_progress(new->runtime);
}

static void replacement_check(const batch_t *batch, int published) {
  domain = 1;
  rinzu_native_completion_wake_t *target = &batch->runtime->native_wake->header;
  size_t references = refs(target);
  assert(references == (published ? 2u : 3u));
  trace("replacement_refs", 0, (intptr_t)target, (int64_t)references);
  rinzu_native_completion_v2_t *carrier = batch->storage[0];
  uintptr_t slot = atomic_load_explicit(&carrier->notification, memory_order_acquire);
  unsigned cancelled = atomic_load_explicit(&carrier->base.cancelled, memory_order_acquire);
  assert(!cancelled && slot == (published ? RINZU_NATIVE_COMPLETION_DONE : (uintptr_t)target));
  trace("replacement_check", 100, (intptr_t)slot, cancelled);
}

static void retired_replacement_check(const batch_t *batch) {
  domain = 1;
  const rinzu_native_completion_v3_t *scalar =
      (const rinzu_native_completion_v3_t *)batch->storage[0];
  uintptr_t slot = atomic_load_explicit(&scalar->completion.notification, memory_order_acquire);
  unsigned cancelled =
      atomic_load_explicit(&scalar->completion.base.cancelled, memory_order_acquire);
  assert(slot == RINZU_NATIVE_COMPLETION_DONE && !cancelled && scalar->result == 7280);
  trace("replacement_retired_check", 100, (intptr_t)slot, cancelled);
}

static void bind_replacement(batch_t *old, batch_t *new, int controlled) {
  rinzu_native_completion_v3_t *storage = (rinzu_native_completion_v3_t *)old->storage[0];
  old->storage[0] = NULL;
  domain = 1;
  trace("storage_handoff", 100, (intptr_t)storage, 0);
  init(new, 1, storage);
  admit(new, 1, controlled);
}

static void reuse_after_origin_dispose(batch_t *old, batch_t *new, int controlled) {
  dispose_batch(old, 0);
  bind_replacement(old, new, controlled);
  replacement_check(new, 0);
  uint64_t epoch = replacement_epoch(new);
  notify_detached(old, 0);
  assert(replacement_epoch(new) == epoch);
  replacement_check(new, 0);
  domain = 0;
  return_reference(0, old->leader);
  assert(replacement_epoch(new) == epoch);
  replacement_check(new, 0);
  publish_replacement(new);
}

static void reuse_across_domains(const char *order, unsigned controlled, int schedule) {
  int notify_before_publication = schedule == 0;
  int replacement_pending = schedule == 2;
  int overlap = schedule == 3;
  int release_during_replacement_notification = schedule == 4;
  int origin_closed_before_handoff = schedule == 5;
  int replacement_destroys_first = schedule >= 6;
  int early_finish = replacement_pending || release_during_replacement_notification ||
                     origin_closed_before_handoff;
  batch_t old = {0};
  batch_t new = {0};
  may_return = 3;
  init(&old, 0, NULL);
  admit(&old, 0, controlled == 0);
  publish(&old, 0);
  rinzu_async_native_progress(old.runtime);

  unsigned settled = 0;
  for (unsigned i = 0; i < 4; ++i) {
    unsigned choice = (unsigned)(order[i] - '0');
    assert(choice < 4 && !(settled & (1u << choice)));
    settled |= 1u << choice;
    unsigned d = choice / 2;
    settle(d ? &new : &old, d, choice % 2, d == controlled, 1);
    if (choice == 0 && !origin_closed_before_handoff) {
      bind_replacement(&old, &new, controlled == 1);
      replacement_check(&new, 0);
      uint64_t epoch = replacement_epoch(&new);
      domain = 0;
      rinzu_async_future_t *alias = old.future[0];
      assert(alias->result_slot == 7183);
      trace("old_receipt", alias->identity, (int64_t)alias->result_slot, 0);
      assert(!rinzu_async_cancel(alias));
      assert(!rinzu_async_timeout(alias));
      if (notify_before_publication)
        notify(&old);
      assert(replacement_epoch(&new) == epoch);
      replacement_check(&new, 0);
      if (overlap)
        overlap_notifications(&old, &new, order[1] == '1');
      else if (release_during_replacement_notification)
        hold_replacement_notification(&new);
      else if (!replacement_pending)
        publish_replacement(&new);
    }
    if (early_finish && i == 1) {
      assert(settled == 3);
      if (origin_closed_before_handoff) {
        reuse_after_origin_dispose(&old, &new, controlled == 1);
        continue;
      }
      uint64_t epoch = replacement_epoch(&new);
      finish(&old, 0, 1);
      assert(replacement_epoch(&new) == epoch);
      replacement_check(&new, !replacement_pending);
      if (replacement_pending) {
        publish_replacement(&new);
      } else {
        end_notification(&new, 1);
        rinzu_async_native_progress(new.runtime);
      }
    }
  }
  if (replacement_destroys_first) {
    replacement_check(&new, 1);
    finish(&new, 1, 0);
    if (schedule == 6)
      notify(&old);
    finish(&old, 0, schedule == 7);
    retired_replacement_check(&new);
  } else {
    if (!early_finish) {
      uint64_t epoch = replacement_epoch(&new);
      finish(&old, 0, !notify_before_publication && !overlap);
      assert(replacement_epoch(&new) == epoch);
      replacement_check(&new, 1);
    }
    finish(&new, 1, 0);
  }
  free(old.storage[1]);
  free(new.storage[0]);
  free(new.storage[1]);
}

static void distinct_domains(const char *order, unsigned controlled, int schedule) {
  int hold = schedule != 0;
  int late = schedule == 2;
  unsigned first = (unsigned)(order[0] - '0') / 2;
  assert(first < 2);
  batch_t owners[2] = {0};
  init(&owners[0], 0, NULL);
  init(&owners[1], 1, NULL);
  admit(&owners[0], 0, controlled == 0);
  admit(&owners[1], 1, controlled == 1);
  assert(owners[0].runtime->native_wake != owners[1].runtime->native_wake);
  for (unsigned d = 0; d < 2; ++d) {
    domain = d;
    owners[d].leader = capture(&owners[d], 0);
  }
  for (unsigned d = 0; d < 2; ++d) {
    domain = d;
    merge(&owners[d], capture(&owners[d], 1), d);
    if (!late || d != first)
      assert(!pthread_create(&owners[d].notifier, NULL, notify, &owners[d]));
  }
  await_notifications(late ? 1u << (1 - first) : 3);

  if (!late)
    end_notification(&owners[first], first);
  if (!hold)
    end_notification(&owners[1 - first], 1 - first);
  for (unsigned d = 0; d < 2; ++d) {
    if (!hold || d == first) {
      domain = d;
      rinzu_async_native_progress(owners[d].runtime);
    }
  }

  unsigned settled = 0;
  for (unsigned i = 0; i < 4; ++i) {
    unsigned choice = (unsigned)(order[i] - '0');
    assert(choice < 4 && !(settled & (1u << choice)));
    settled |= 1u << choice;
    unsigned d = choice / 2;
    unsigned g = choice % 2;
    settle(&owners[d], d, g, controlled == d, 0);
    if (owners[d].remaining == 0) {
      finish(&owners[d], d, late && d == first);
      if (hold && d == first) {
        assert(refs(owners[1 - d].leader) == 2);
        end_notification(&owners[1 - d], 1 - d);
        domain = 1 - d;
        rinzu_async_native_progress(owners[1 - d].runtime);
      }
    }
  }
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IOLBF, 0);
  alarm(30);
  assert((argc == 4 || argc == 5) && strlen(argv[1]) == 4);
  unsigned controlled = (unsigned)(argv[2][0] - '0');
  int schedule = argv[3][0] - '0';
  assert(controlled <= 2 && schedule >= 0);
  if (argc == 5) {
    assert(!strcmp(argv[4], "reuse") && schedule <= 7);
    reuse_across_domains(argv[1], controlled, schedule);
  } else {
    assert(schedule <= 2);
    distinct_domains(argv[1], controlled, schedule);
  }
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  return 0;
}
