#ifndef RINZU_RUNTIME_H
#define RINZU_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#if defined(RINZU_RUNTIME_BUILD)
#define RINZU_API __declspec(dllexport)
#else
#define RINZU_API __declspec(dllimport)
#endif
#define RINZU_CALL __cdecl
#else
#define RINZU_API __attribute__((visibility("default")))
#define RINZU_CALL
#endif

#if defined(__cplusplus)
extern "C" {
#endif

#define RINZU_ABI_VERSION_MAJOR UINT32_C(1)
#define RINZU_ABI_VERSION_MINOR UINT32_C(1)
#define RINZU_ABI_VERSION_PATCH UINT32_C(0)
#define RINZU_ABI_VERSION                                                                          \
  ((RINZU_ABI_VERSION_MAJOR << 24) | (RINZU_ABI_VERSION_MINOR << 12) | RINZU_ABI_VERSION_PATCH)

typedef int32_t rinzu_status_t;

#define RINZU_STATUS_OK INT32_C(0)
#define RINZU_STATUS_INVALID_ARGUMENT INT32_C(1)
#define RINZU_STATUS_ABI_MISMATCH INT32_C(2)
#define RINZU_STATUS_OUT_OF_MEMORY INT32_C(3)
#define RINZU_STATUS_WRONG_THREAD INT32_C(4)
#define RINZU_STATUS_UNSUPPORTED INT32_C(5)
#define RINZU_STATUS_PANIC INT32_C(6)
#define RINZU_STATUS_QUEUE_FULL INT32_C(7)

#define RINZU_HOST_POST_ACCEPTED INT32_C(0)
#define RINZU_HOST_POST_CLOSED INT32_C(1)
#define RINZU_HOST_POST_FAILURE INT32_C(2)

#define RINZU_HOST_NESTED_ALL_MODES UINT32_C(0)
#define RINZU_HOST_NESTED_DECLARED_MODES UINT32_C(1)
#define RINZU_HOST_NESTED_UNSUPPORTED UINT32_C(2)

#define RINZU_HOST_RETURN_RESUME_MANAGED UINT32_C(0)

typedef void(RINZU_CALL *rinzu_host_owner_turn_callback_t)(void *context, uint64_t generation);
typedef rinzu_status_t(RINZU_CALL *rinzu_host_prepare_callback_t)(
    void *context, rinzu_host_owner_turn_callback_t owner_turn, void *owner_context,
    uint64_t generation, void **out_ready_token);
typedef int32_t(RINZU_CALL *rinzu_host_post_owner_turn_callback_t)(void *context, void *ready_token,
                                                                   uint64_t generation);
typedef void(RINZU_CALL *rinzu_host_deactivate_callback_t)(void *context, void *ready_token,
                                                           uint64_t generation);
typedef void(RINZU_CALL *rinzu_host_work_callback_t)(void *context);

#define RINZU_NATIVE_PACKAGE_ENTRY_V1 "rinzu_native_package_v1"

typedef struct rinzu_native_import_v1 {
  uint64_t import_key;
  const char *canonical_key;
  const char *signature;
  uintptr_t address;
} rinzu_native_import_v1_t;

typedef int64_t (*rinzu_native_invoke_handler_v1_t)(void *runtime_context, uint64_t handler);

typedef struct rinzu_native_invocation_context_v1 {
  uint32_t struct_size;
  uint32_t abi_version;
  void *runtime_context;
  rinzu_native_invoke_handler_v1_t invoke_handler;
} rinzu_native_invocation_context_v1_t;

typedef rinzu_status_t (*rinzu_native_function_v1_t)(
    const rinzu_native_invocation_context_v1_t *invocation, const int64_t *arguments,
    size_t argument_count, int64_t *out_result);

typedef struct rinzu_native_package_descriptor_v1 {
  uint32_t struct_size;
  uint32_t abi_version;
  const char *package_name;
  const uint8_t *metadata_digest;
  uint32_t metadata_digest_size;
  uint32_t reserved;
  size_t import_count;
  const rinzu_native_import_v1_t *imports;
} rinzu_native_package_descriptor_v1_t;

typedef const rinzu_native_package_descriptor_v1_t *(RINZU_CALL *rinzu_native_package_entry_v1_t)(
    void);

/* Separate source-package ABI. Never reinterpret the v1 descriptor. */
#define RINZU_SOURCE_PACKAGE_ENTRY_V2 "rinzu_source_package_v2"
#define RINZU_SOURCE_PACKAGE_MAJOR UINT16_C(2)
#define RINZU_SOURCE_PACKAGE_MINOR UINT16_C(5)
#define RINZU_CALLBACK_RULE_VERSION UINT32_C(1)
#define RINZU_TEMPLATE_LAYOUT_VERSION UINT32_C(2)
#define RINZU_CLOSED_SERVICE_REGISTRY_VERSION UINT32_C(2)

typedef struct rinzu_source_import_v2 {
  uint8_t import_key[32];
  const char *symbol;
  const void *address;
} rinzu_source_import_v2_t;

typedef struct rinzu_source_package_descriptor_v2 {
  uint32_t struct_size;
  uint16_t major;
  uint16_t minor;
  const uint8_t *descriptor;
  size_t descriptor_len;
  size_t import_count;
  const rinzu_source_import_v2_t *imports;
} rinzu_source_package_descriptor_v2_t;

typedef const rinzu_source_package_descriptor_v2_t *(RINZU_CALL *rinzu_source_package_entry_v2_t)(
    void);

typedef struct rinzu_host_loop_profile_v1 {
  uint32_t struct_size;
  uint32_t abi_version;
  void *context;
  rinzu_host_prepare_callback_t prepare;
  rinzu_host_post_owner_turn_callback_t post_owner_turn;
  rinzu_host_deactivate_callback_t deactivate;
  uint32_t nested_delivery;
  uint32_t return_policy;
  uint32_t turn_budget;
  uint32_t reserved;
} rinzu_host_loop_profile_v1_t;

/*
 * Every public struct begins with struct_size and abi_version. Callers zero
 * initialize the whole struct, set both fields, then pass it by const pointer.
 * A newer runtime may read only the prefix described by struct_size.
 */
typedef struct rinzu_runtime_options {
  uint32_t struct_size;
  uint32_t abi_version;
  uint64_t flags;
} rinzu_runtime_options_t;

typedef struct rinzu_abi_descriptor {
  uint32_t struct_size;
  uint32_t abi_version;
  uint32_t pointer_width_bits;
  uint32_t pointer_align_bytes;
  uint32_t runtime_options_size;
  uint32_t runtime_options_align;
  uint32_t status_size;
  uint32_t host_loop_profile_v1_size;
  uint32_t host_loop_profile_v1_align;
  uint32_t native_import_v1_size;
  uint32_t native_import_v1_align;
  uint32_t native_package_descriptor_v1_size;
  uint32_t native_package_descriptor_v1_align;
  uint32_t native_invocation_context_v1_size;
  uint32_t native_invocation_context_v1_align;
  uint32_t reserved;
} rinzu_abi_descriptor_t;

typedef struct rinzu_runtime rinzu_runtime_t;
typedef struct rinzu_handle rinzu_handle_t;
typedef struct rinzu_host_session rinzu_host_session_t;

/*
 * A callback uses the platform C ABI selected by RINZU_CALL. It may be called
 * from any thread, must return promptly, and must not let a C++ exception,
 * Objective-C exception, Rust panic, or managed exception cross this boundary.
 * `context` remains owned by the caller until the runtime is destroyed or the
 * callback is replaced and the replacing call returns.
 */
typedef void(RINZU_CALL *rinzu_wake_callback_t)(void *context);

RINZU_API uint32_t RINZU_CALL rinzu_abi_version(void);

RINZU_API rinzu_status_t RINZU_CALL rinzu_abi_query(rinzu_abi_descriptor_t *out_descriptor);

RINZU_API rinzu_status_t RINZU_CALL
rinzu_runtime_options_init(rinzu_runtime_options_t *out_options);

RINZU_API rinzu_status_t RINZU_CALL rinzu_runtime_create(const rinzu_runtime_options_t *options,
                                                         rinzu_runtime_t **out_runtime);

RINZU_API void RINZU_CALL rinzu_runtime_destroy(rinzu_runtime_t *runtime);

RINZU_API rinzu_status_t RINZU_CALL rinzu_runtime_set_wake_callback(rinzu_runtime_t *runtime,
                                                                    rinzu_wake_callback_t callback,
                                                                    void *context);

/*
 * A Host Session is created and entered on its owner thread. enqueue may be
 * called concurrently; accepted callback/context pairs remain borrowed until
 * callback delivery or leave returns. leave and destroy are owner-thread-only.
 * The profile and all of its borrowed callback state must outlive the session.
 */
RINZU_API rinzu_status_t RINZU_CALL
rinzu_host_session_create(rinzu_runtime_t *runtime, const rinzu_host_loop_profile_v1_t *profile,
                          rinzu_host_session_t **out_session);

RINZU_API rinzu_status_t RINZU_CALL rinzu_host_session_enter(rinzu_host_session_t *session,
                                                             uint64_t *out_generation);

RINZU_API rinzu_status_t RINZU_CALL rinzu_host_session_enqueue(rinzu_host_session_t *session,
                                                               rinzu_host_work_callback_t callback,
                                                               void *context);

RINZU_API rinzu_status_t RINZU_CALL rinzu_host_session_leave(rinzu_host_session_t *session);

RINZU_API rinzu_status_t RINZU_CALL rinzu_host_session_destroy(rinzu_host_session_t *session);

/*
 * rinzu_handle_t is opaque and always represents one owned stable-handle
 * reference. A successful retain creates one additional owned reference in
 * out_handle. release consumes exactly one owned reference. Borrowing the raw
 * pointer, copying it without retain, or using it after release is invalid.
 * Null is never an owned handle and release(NULL) is a no-op.
 */
RINZU_API rinzu_status_t RINZU_CALL rinzu_handle_retain(const rinzu_handle_t *handle,
                                                        rinzu_handle_t **out_handle);

RINZU_API void RINZU_CALL rinzu_handle_release(rinzu_handle_t *handle);

#if defined(__cplusplus)
} /* extern "C" */
#endif

#endif /* RINZU_RUNTIME_H */
