#ifndef RINZU_GC_POLICY_H
#define RINZU_GC_POLICY_H
#include <stddef.h>
#include <stdint.h>

/* Domain-local pacing, independent of optional resource accounting. Allocation
   only charges the budget; collection waits until typed roots are published. */
#define RINZU_GC_MIN_BUDGET ((size_t)256 * 1024)
typedef struct {
  size_t allocated, budget;
  int stress;
} rinzu_gc_policy_t;
static inline size_t rinzu_gc_add(size_t left, size_t right) {
  return right > SIZE_MAX - left ? SIZE_MAX : left + right;
}
static inline rinzu_gc_policy_t rinzu_gc_policy_init(int stress) {
  return (rinzu_gc_policy_t){.budget = RINZU_GC_MIN_BUDGET, .stress = stress};
}
static inline void rinzu_gc_allocated(rinzu_gc_policy_t *policy, size_t bytes) {
  policy->allocated = rinzu_gc_add(policy->allocated, bytes);
}
static inline int rinzu_gc_due(const rinzu_gc_policy_t *policy) {
  return policy->stress || policy->allocated >= policy->budget;
}
static inline void rinzu_gc_collected(rinzu_gc_policy_t *policy, size_t live, size_t roots) {
  size_t work = rinzu_gc_add(live, roots);
  policy->budget = work < RINZU_GC_MIN_BUDGET ? RINZU_GC_MIN_BUDGET : work;
  policy->allocated = 0;
}
#endif
