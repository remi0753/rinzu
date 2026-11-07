#ifndef RINZU_NATIVE_USERDATA_H
#define RINZU_NATIVE_USERDATA_H
#include "rinzu/thread_identity.h"
#include "rinzu/managed_composite.h"

/* Token bits are never-reused numbers, not addresses of allocated metadata.
   Only the live call owns the lookup entry; no native input is dereferenced. */
typedef struct rinzu_userdata_argument {
  uintptr_t token;
  size_t type;
  uint64_t value;
  struct rinzu_userdata_argument *next;
} rinzu_userdata_argument_t;
typedef struct rinzu_userdata_admission {
  uint64_t identity;
  struct rinzu_userdata_admission *next;
} rinzu_userdata_admission_t;
typedef struct rinzu_userdata_scope {
  uint64_t identity;
  unsigned phase, result;
  size_t result_type;
  rinzu_userdata_argument_t *arguments;
  rinzu_userdata_admission_t *admissions;
  rinzu_managed_roots_t roots;
  rinzu_managed_root_set_t attached;
  struct rinzu_userdata_scope *next;
} rinzu_userdata_scope_t;
typedef struct {
  rinzu_managed_data_t *owner;
  pthread_t thread;
  uint64_t identity;
  rinzu_userdata_scope_t *scopes;
  size_t live_scopes, tokens, roots, in_flight, created, released;
} rinzu_userdata_registry_t;
typedef struct {
  uint64_t scope, identity;
} rinzu_userdata_invocation_t;
typedef struct {
  size_t tokens, closed, token_bytes;
} rinzu_userdata_process_stats_t;
enum { RINZU_USERDATA_OK, RINZU_USERDATA_NULL };
enum { RINZU_USERDATA_PREPARING, RINZU_USERDATA_OPEN, RINZU_USERDATA_SETTLING };

static _Atomic size_t rinzu_userdata_total;
static inline void rinzu_userdata_fatal(void) {
  rinzu_interop_fatal();
}
static inline uint64_t rinzu_userdata_identity(void) {
#ifdef RINZU_CONTEXT_IDENTITY
  uint64_t value = RINZU_CONTEXT_IDENTITY();
  if (!value || value > UINTPTR_MAX)
    rinzu_userdata_fatal();
  return value;
#else
  static _Atomic uint64_t next = 1;
  uint64_t value = atomic_load_explicit(&next, memory_order_relaxed);
  do {
    if (value == UINT64_MAX || value > UINTPTR_MAX)
      rinzu_resource_fatal();
  } while (!atomic_compare_exchange_weak_explicit(&next, &value, value + 1, memory_order_relaxed,
                                                  memory_order_relaxed));
  return value;
#endif
}
static inline void rinzu_userdata_owner(rinzu_userdata_registry_t *registry,
                                        rinzu_managed_data_t *data) {
  if (!registry || !data ||
      (data->owner_initialized && !rinzu_thread_equal(data->owner, pthread_self())))
    rinzu_userdata_fatal();
  if (registry->owner &&
      (registry->owner != data || !rinzu_thread_equal(registry->thread, pthread_self())))
    rinzu_userdata_fatal();
  rinzu_managed_native_owner(data);
  if (!registry->owner) {
    registry->owner = data;
    registry->thread = pthread_self();
    registry->identity = rinzu_userdata_identity();
  }
}
static inline void rinzu_userdata_mapping(rinzu_managed_data_t *data, size_t type) {
  if (!data->schema || type >= data->schema->count ||
      (data->schema->types[type].kind != RINZU_VALUE_STATE &&
       data->schema->types[type].kind != RINZU_VALUE_RECORD &&
       data->schema->types[type].kind != RINZU_VALUE_ENUM))
    rinzu_userdata_fatal();
}
static inline rinzu_userdata_scope_t *rinzu_userdata_scope(rinzu_userdata_registry_t *registry,
                                                           uint64_t identity) {
  for (rinzu_userdata_scope_t *scope = registry->scopes; scope; scope = scope->next)
    if (scope->identity == identity)
      return scope;
  rinzu_userdata_fatal();
  return NULL;
}
static inline uint64_t rinzu_userdata_enter(rinzu_userdata_registry_t *registry,
                                            rinzu_managed_data_t *data) {
  rinzu_userdata_owner(registry, data);
  if (registry->live_scopes == SIZE_MAX)
    rinzu_resource_fatal();
  rinzu_userdata_scope_t *scope = rinzu_failure_allocate(sizeof(*scope));
  scope->identity = rinzu_userdata_identity();
  scope->next = registry->scopes;
  rinzu_managed_enter(&scope->roots);
  rinzu_managed_attach_roots(data, &scope->attached, &scope->roots);
  registry->scopes = scope;
  registry->live_scopes++;
  return scope->identity;
}
static inline const void *rinzu_userdata_prepare(rinzu_userdata_registry_t *registry,
                                                 rinzu_managed_data_t *data, uint64_t identity,
                                                 size_t type, uint64_t value) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_mapping(data, type);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_PREPARING ||
      rinzu_managed_slot_valid(data, type, (rinzu_managed_slot_t){value, 1}))
    rinzu_userdata_fatal();
  rinzu_managed_frame_t *frame = scope->roots.current;
  if (!frame)
    rinzu_interop_fatal();
  if (frame->count >= SIZE_MAX / sizeof(*frame->roots) || registry->tokens == SIZE_MAX ||
      registry->created == SIZE_MAX || registry->roots == SIZE_MAX)
    rinzu_resource_fatal();
  uintptr_t token = (uintptr_t)rinzu_userdata_identity();
  rinzu_userdata_argument_t *argument = rinzu_failure_allocate(sizeof(*argument));
  uint64_t *roots = rinzu_managed_reserve_roots(&scope->roots, frame->count + 1);
  roots[frame->count] = value;
  rinzu_managed_set_count(&scope->roots, frame->count + 1);
  scope->roots.publications++;
  *argument = (rinzu_userdata_argument_t){token, type, value, scope->arguments};
  scope->arguments = argument;
  size_t total = atomic_load_explicit(&rinzu_userdata_total, memory_order_relaxed);
  do {
    if (total >= SIZE_MAX / sizeof(*argument))
      rinzu_resource_fatal();
  } while (!atomic_compare_exchange_weak_explicit(&rinzu_userdata_total, &total, total + 1,
                                                  memory_order_relaxed, memory_order_relaxed));
  registry->tokens++;
  registry->roots++;
  registry->created++;
  return (const void *)token;
}
static inline void rinzu_userdata_open(rinzu_userdata_registry_t *registry,
                                       rinzu_managed_data_t *data, uint64_t identity) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_PREPARING)
    rinzu_userdata_fatal();
  scope->phase = RINZU_USERDATA_OPEN;
}
static inline void rinzu_userdata_expect_result(rinzu_userdata_registry_t *registry,
                                                rinzu_managed_data_t *data, uint64_t identity,
                                                size_t type) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_mapping(data, type);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_PREPARING || scope->result)
    rinzu_userdata_fatal();
  scope->result = 1;
  scope->result_type = type;
}
static inline int rinzu_userdata_receive_result(rinzu_userdata_registry_t *registry,
                                                rinzu_managed_data_t *data, uint64_t identity,
                                                size_t type, const void *pointer,
                                                uint64_t *output) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_SETTLING || scope->admissions || scope->result != 1 ||
      scope->result_type != type || !output)
    rinzu_userdata_fatal();
  rinzu_userdata_mapping(data, type);
  scope->result = 2;
  if (!pointer)
    return RINZU_USERDATA_NULL;
  rinzu_userdata_argument_t *argument = scope->arguments;
  while (argument && argument->token != (uintptr_t)pointer)
    argument = argument->next;
  if (!argument || argument->type != type ||
      rinzu_managed_slot_valid(data, type, (rinzu_managed_slot_t){argument->value, 1}))
    rinzu_userdata_fatal();
  *output = argument->value;
  return RINZU_USERDATA_OK;
}
static inline rinzu_userdata_invocation_t rinzu_userdata_admit(rinzu_userdata_registry_t *registry,
                                                               rinzu_managed_data_t *data,
                                                               uint64_t identity) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_OPEN)
    rinzu_userdata_fatal();
  if (registry->in_flight == SIZE_MAX)
    rinzu_resource_fatal();
  rinzu_userdata_admission_t *admission = rinzu_failure_allocate(sizeof(*admission));
  *admission = (rinzu_userdata_admission_t){rinzu_userdata_identity(), scope->admissions};
  scope->admissions = admission;
  registry->in_flight++;
  return (rinzu_userdata_invocation_t){identity, admission->identity};
}
static inline int rinzu_userdata_resolve(rinzu_userdata_registry_t *registry,
                                         rinzu_managed_data_t *data,
                                         rinzu_userdata_invocation_t invocation, size_t type,
                                         const void *pointer, uint64_t *output) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, invocation.scope);
  rinzu_userdata_admission_t *admission = scope->admissions;
  while (admission && admission->identity != invocation.identity)
    admission = admission->next;
  if (!admission || scope->phase == RINZU_USERDATA_PREPARING || !output)
    rinzu_userdata_fatal();
  rinzu_userdata_mapping(data, type);
  if (!pointer)
    return RINZU_USERDATA_NULL;
  rinzu_userdata_argument_t *argument = scope->arguments;
  while (argument && argument->token != (uintptr_t)pointer)
    argument = argument->next;
  if (!argument || argument->type != type ||
      rinzu_managed_slot_valid(data, type, (rinzu_managed_slot_t){argument->value, 1}))
    rinzu_userdata_fatal();
  *output = argument->value;
  return RINZU_USERDATA_OK;
}
static inline void rinzu_userdata_release_scope(rinzu_userdata_registry_t *registry,
                                                rinzu_managed_data_t *data,
                                                rinzu_userdata_scope_t *scope) {
  if (scope->admissions || !registry->live_scopes)
    rinzu_userdata_fatal();
  rinzu_userdata_scope_t **slot = &registry->scopes;
  while (*slot && *slot != scope)
    slot = &(*slot)->next;
  if (!*slot)
    rinzu_userdata_fatal();
  rinzu_userdata_argument_t *argument = scope->arguments;
  while (argument) {
    if (!argument->token || !registry->tokens || !registry->roots || registry->released == SIZE_MAX)
      rinzu_userdata_fatal();
    if (!atomic_fetch_sub_explicit(&rinzu_userdata_total, 1, memory_order_relaxed))
      rinzu_userdata_fatal();
    registry->tokens--;
    registry->roots--;
    registry->released++;
    rinzu_userdata_argument_t *next = argument->next;
    free(argument);
    argument = next;
  }
  rinzu_managed_leave(&scope->roots);
  rinzu_managed_detach_roots(data, &scope->attached);
  rinzu_managed_dispose(&scope->roots);
  *slot = scope->next;
  registry->live_scopes--;
  free(scope);
}
static inline void rinzu_userdata_discard_prepared(rinzu_userdata_registry_t *registry,
                                                   rinzu_managed_data_t *data, uint64_t identity) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_PREPARING)
    rinzu_userdata_fatal();
  rinzu_userdata_release_scope(registry, data, scope);
}
static inline void rinzu_userdata_finish_result(rinzu_userdata_registry_t *registry,
                                                rinzu_managed_data_t *data, uint64_t identity) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_SETTLING || scope->admissions || !scope->result)
    rinzu_userdata_fatal();
  rinzu_userdata_release_scope(registry, data, scope);
}
static inline void rinzu_userdata_returned(rinzu_userdata_registry_t *registry,
                                           rinzu_managed_data_t *data, uint64_t identity) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, identity);
  if (scope->phase != RINZU_USERDATA_OPEN)
    rinzu_userdata_fatal();
  scope->phase = RINZU_USERDATA_SETTLING;
  if (!scope->admissions && !scope->result)
    rinzu_userdata_release_scope(registry, data, scope);
}
static inline void rinzu_userdata_settle(rinzu_userdata_registry_t *registry,
                                         rinzu_managed_data_t *data,
                                         rinzu_userdata_invocation_t invocation) {
  rinzu_userdata_owner(registry, data);
  rinzu_userdata_scope_t *scope = rinzu_userdata_scope(registry, invocation.scope);
  rinzu_userdata_admission_t **slot = &scope->admissions;
  while (*slot && (*slot)->identity != invocation.identity)
    slot = &(*slot)->next;
  if (!*slot || !registry->in_flight)
    rinzu_userdata_fatal();
  rinzu_userdata_admission_t *admission = *slot;
  *slot = admission->next;
  free(admission);
  registry->in_flight--;
  if (scope->phase == RINZU_USERDATA_SETTLING && !scope->admissions && !scope->result)
    rinzu_userdata_release_scope(registry, data, scope);
}
static inline void rinzu_userdata_finish(rinzu_userdata_registry_t *registry,
                                         rinzu_managed_data_t *data) {
  rinzu_userdata_owner(registry, data);
  if (registry->scopes || registry->live_scopes || registry->tokens || registry->roots ||
      registry->in_flight || registry->created != registry->released)
    rinzu_userdata_fatal();
}
static inline rinzu_userdata_process_stats_t rinzu_userdata_process_stats(void) {
  size_t total = atomic_load_explicit(&rinzu_userdata_total, memory_order_relaxed);
  rinzu_userdata_process_stats_t stats = {total, 0, total * sizeof(rinzu_userdata_argument_t)};
  return stats;
}
#endif
