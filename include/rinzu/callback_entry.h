#ifndef RINZU_CALLBACK_ENTRY_H
#define RINZU_CALLBACK_ENTRY_H
#include <stddef.h>
#include <stdint.h>

/* Private source-package callback backend. Context is carried by its immutable entry.
   Entry addresses remain allocated until process exit. Full closed controls
   may retire only under the separate whole-capability/physical-reader proof. */
typedef struct rinzu_callback_control rinzu_callback_control_t;
typedef struct rinzu_foreign_queue rinzu_foreign_queue_t;
typedef struct rinzu_activity_board rinzu_activity_board_t;
typedef uint64_t (*rinzu_callback_pointer_u64_handler_t)(void *, const unsigned char *);
typedef uint64_t (*rinzu_callback_pointer_u64_t)(const unsigned char *);
typedef struct {
  rinzu_callback_control_t *control;
  rinzu_callback_pointer_u64_t entry;
} rinzu_callback_pointer_u64_entry_t;
/* The generic precompiled bridge captures arguments before C can clobber them.
   Stack points to the original native caller's parameter area. */
typedef struct {
  uint64_t integers[8], floats[8];
  const unsigned char *stack;
} rinzu_callback_frame_t;
typedef uint64_t (*rinzu_callback_frame_handler_t)(void *, const rinzu_callback_frame_t *);
typedef struct {
  rinzu_callback_control_t *control;
  const void *entry;
} rinzu_callback_frame_entry_t;
typedef struct {
  size_t integers, floats, stack;
} rinzu_callback_abi_cursor_t;
enum { RINZU_CALLBACK_POINTER = 11 }; /* Scalar tags 0..10 agree with the closed ABI. */
typedef struct {
  size_t entries, blocks, virtual_bytes, control_bytes, live_connections, in_flight, invocations;
  /* Process retention includes full closed controls and block-list metadata.
     Diagnostic copies belong to call connections and return to zero at close. */
  size_t closed_entries, tombstone_bytes, block_metadata_bytes;
  size_t live_diagnostics, diagnostic_bytes;
  /* Retired NULL slots remain in the RW reservation; released control bytes
     are cumulative physical heap frees, never additional retained storage. */
  size_t retired_entries, retired_slot_bytes, released_control_bytes;
} rinzu_callback_stats_t;

rinzu_callback_pointer_u64_entry_t
rinzu_callback_pointer_u64_prepare(rinzu_callback_pointer_u64_handler_t handler, void *context);
rinzu_callback_frame_entry_t rinzu_callback_frame_prepare(rinzu_callback_frame_handler_t handler,
                                                          void *context);
/* Production call ledgers keep admission Prepared until physical native entry.
   Closing a Prepared entry releases its connection without ever admitting it. */
rinzu_callback_pointer_u64_entry_t
rinzu_callback_pointer_u64_prepare_deferred(rinzu_callback_pointer_u64_handler_t handler,
                                            void *context);
rinzu_callback_frame_entry_t
rinzu_callback_frame_prepare_deferred(rinzu_callback_frame_handler_t handler, void *context);
void rinzu_callback_entry_open(rinzu_callback_control_t *control);
/* Private generated signature/progress binding. Queue is pinned through close
   and every admitted native result receipt. No public delivery modifier. */
void rinzu_callback_entry_bind_foreign(rinzu_callback_control_t *, rinzu_foreign_queue_t *,
                                       uint64_t generation, int eligible);
/* Copies native-only metadata while Prepared. Origin pins the board until
   close and every admitted result receipt have settled. */
void rinzu_callback_entry_bind_diagnostics(rinzu_callback_control_t *, rinzu_activity_board_t *,
                                           const char *symbol, uint64_t file, uint32_t start_byte,
                                           uint32_t end_byte, uint64_t owner, uint64_t depth);
int rinzu_callback_foreign_drain_one(rinzu_foreign_queue_t *);
int rinzu_callback_foreign_executing(void);
/* Decode a validated fixed signature. Narrow register/packed stack parameters
   are canonicalized; Bool's byte is retained for the receiver's Conversion gate. */
uint64_t rinzu_callback_frame_word(const rinzu_callback_frame_t *frame,
                                   rinzu_callback_abi_cursor_t *cursor, unsigned kind);
/* Called on the preparing thread. The caller pins context until close returns
   true or an admitted invocation finishes and settled becomes true. */
int rinzu_callback_entry_close(rinzu_callback_control_t *control);
int rinzu_callback_entry_settled(rinzu_callback_control_t *control);
/* Origin-only once owner. Caller MUST prove all future callback capabilities
   closed and all physical/pre-admission/receipt readers returned. CLOSED and
   in_flight==0 alone do not prove this. Clears *owner on success; NULL is a
   completed no-op. RX/RW addresses are never reused. */
int rinzu_callback_entry_retire(rinzu_callback_control_t **owner);
rinzu_callback_stats_t rinzu_callback_entry_stats(void);
#ifdef RINZU_CALLBACK_TESTING
void rinzu_callback_test_fail_allocation_after(size_t successes);
void rinzu_callback_test_fail_mapping(int category);
void rinzu_callback_test_merge_control_region(void);
#endif
#endif
