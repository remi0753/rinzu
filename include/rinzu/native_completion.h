#ifndef RINZU_NATIVE_COMPLETION_H
#define RINZU_NATIVE_COMPLETION_H
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
/* SDK-owned, pinned through every admitted await and physical publication.
   The producer publishes done only after its last payload access. Cancellation
   is a request; the consumer still waits for done. No allocator is inferred. */
typedef struct {
  uint32_t version, size;
  _Atomic uint32_t done, cancelled;
} rinzu_native_completion_t;
_Static_assert(sizeof(rinzu_native_completion_t) == 16, "completion layout");
static inline void rinzu_native_completion_init(rinzu_native_completion_t *completion) {
  completion->version = 1;
  completion->size = sizeof(*completion);
  atomic_init(&completion->done, 0);
  atomic_init(&completion->cancelled, 0);
}
/* Optional v2 extension. The v1 prefix and initializer above are unchanged.
   One runtime notification domain may register a retained, native-only target.
   Registration transfers that reference only when its CAS wins. Repeated waits
   in the same runtime share the registration. A target must remain callable
   after runtime teardown (then no-op) until the producer releases its reference.
   No managed value or SDK carrier is captured by the target. */
typedef struct rinzu_native_completion_wake rinzu_native_completion_wake_t;
struct rinzu_native_completion_wake {
  void (*notify)(rinzu_native_completion_wake_t *);
  void (*release)(rinzu_native_completion_wake_t *);
};
typedef struct {
  rinzu_native_completion_t base;
  /* 0: unregistered; aligned target pointer: registered; 1: physically done. */
  _Atomic uintptr_t notification;
} rinzu_native_completion_v2_t;
/* Additive scalar-result receipt. The producer writes result before its LAST
   notification exchange. A runtime copies it during physical publication,
   before announcing physical_done; a later await never touches this carrier.
   SDKs may embed the same prefix/result layout in their own pinned record. */
typedef struct {
  rinzu_native_completion_v2_t completion;
  int64_t result;
} rinzu_native_completion_v3_t;
_Static_assert(offsetof(rinzu_native_completion_v3_t, result) ==
                   sizeof(rinzu_native_completion_v2_t), "scalar completion prefix");
#define RINZU_NATIVE_COMPLETION_DONE ((uintptr_t)1)
static inline void rinzu_native_completion_v2_init(rinzu_native_completion_v2_t *completion) {
  rinzu_native_completion_init(&completion->base);
  completion->base.version = 2;
  completion->base.size = sizeof(*completion);
  atomic_init(&completion->notification, 0);
}
static inline int rinzu_native_completion_valid(const rinzu_native_completion_t *completion) {
  return completion &&
         ((completion->version == 1 && completion->size == sizeof(*completion)) ||
          ((completion->version == 2 || completion->version == 3) &&
           completion->size == (completion->version == 2 ? sizeof(rinzu_native_completion_v2_t)
                                                        : sizeof(rinzu_native_completion_v3_t)) &&
           !((uintptr_t)completion % _Alignof(rinzu_native_completion_v2_t))));
}
static inline int rinzu_native_completion_is_done(const rinzu_native_completion_t *completion) {
  if (completion->version == 2 || completion->version == 3)
    return atomic_load_explicit(&((const rinzu_native_completion_v2_t *)completion)->notification,
                                memory_order_acquire) == RINZU_NATIVE_COMPLETION_DONE;
  return atomic_load_explicit(&completion->done, memory_order_acquire) != 0;
}
static inline int64_t rinzu_native_completion_transfer_result(const rinzu_native_completion_t *completion) {
  if (completion->version != 3 || completion->size != sizeof(rinzu_native_completion_v3_t) ||
      !rinzu_native_completion_is_done(completion))
    __builtin_trap();
  int64_t result;
  /* Byte access also permits a provider's own record with this exact prefix,
     without violating C's struct aliasing rules. Publication supplies acquire. */
  memcpy(&result, (const unsigned char *)completion + offsetof(rinzu_native_completion_v3_t, result), sizeof(result));
  return result;
}
/* The exchange is the producer's LAST carrier access and the v2 physical
   publication. A consumer may retire/reuse the carrier immediately afterward.
   Capture the target from that exchange, then use only its retained reference.
   base.done is diagnostic for v2; it never authorizes physical retirement. */
static inline void rinzu_native_completion_v2_publish(rinzu_native_completion_v2_t *completion) {
  atomic_store_explicit(&completion->base.done, 1, memory_order_release);
  uintptr_t before = atomic_exchange_explicit(&completion->notification,
                                               RINZU_NATIVE_COMPLETION_DONE, memory_order_acq_rel);
  if (before > RINZU_NATIVE_COMPLETION_DONE) {
    rinzu_native_completion_wake_t *target = (rinzu_native_completion_wake_t *)before;
    target->notify(target);
    target->release(target);
  }
}
#endif
