#include "rinzu/runtime.h"

#include <stddef.h>

_Static_assert(sizeof(rinzu_status_t) == 4, "rinzu_status_t width");
_Static_assert(sizeof(rinzu_runtime_options_t) == 16, "rinzu_runtime_options_t size");
_Static_assert(_Alignof(rinzu_runtime_options_t) == 8, "rinzu_runtime_options_t alignment");
_Static_assert(offsetof(rinzu_runtime_options_t, flags) == 8,
               "rinzu_runtime_options_t flags offset");
_Static_assert(sizeof(rinzu_abi_descriptor_t) == 64, "rinzu_abi_descriptor_t size");
_Static_assert(_Alignof(rinzu_abi_descriptor_t) == 4, "rinzu_abi_descriptor_t alignment");
_Static_assert(sizeof(rinzu_host_loop_profile_v1_t) == 56, "rinzu_host_loop_profile_v1_t size");
_Static_assert(_Alignof(rinzu_host_loop_profile_v1_t) == 8,
               "rinzu_host_loop_profile_v1_t alignment");
_Static_assert(sizeof(rinzu_native_import_v1_t) == 32, "rinzu_native_import_v1_t size");
_Static_assert(_Alignof(rinzu_native_import_v1_t) == 8, "rinzu_native_import_v1_t alignment");
_Static_assert(sizeof(rinzu_native_package_descriptor_v1_t) == 48,
               "rinzu_native_package_descriptor_v1_t size");
_Static_assert(_Alignof(rinzu_native_package_descriptor_v1_t) == 8,
               "rinzu_native_package_descriptor_v1_t alignment");
_Static_assert(sizeof(rinzu_native_invocation_context_v1_t) == 24,
               "rinzu_native_invocation_context_v1_t size");
_Static_assert(_Alignof(rinzu_native_invocation_context_v1_t) == 8,
               "rinzu_native_invocation_context_v1_t alignment");

uint32_t rinzu_c_abi_version(void) {
  return RINZU_ABI_VERSION;
}
size_t rinzu_c_status_size(void) {
  return sizeof(rinzu_status_t);
}
size_t rinzu_c_runtime_options_size(void) {
  return sizeof(rinzu_runtime_options_t);
}
size_t rinzu_c_runtime_options_align(void) {
  return _Alignof(rinzu_runtime_options_t);
}
size_t rinzu_c_descriptor_size(void) {
  return sizeof(rinzu_abi_descriptor_t);
}
size_t rinzu_c_descriptor_align(void) {
  return _Alignof(rinzu_abi_descriptor_t);
}
size_t rinzu_c_host_loop_profile_size(void) {
  return sizeof(rinzu_host_loop_profile_v1_t);
}
size_t rinzu_c_host_loop_profile_align(void) {
  return _Alignof(rinzu_host_loop_profile_v1_t);
}
size_t rinzu_c_native_import_size(void) {
  return sizeof(rinzu_native_import_v1_t);
}
size_t rinzu_c_native_import_align(void) {
  return _Alignof(rinzu_native_import_v1_t);
}
size_t rinzu_c_native_package_descriptor_size(void) {
  return sizeof(rinzu_native_package_descriptor_v1_t);
}
size_t rinzu_c_native_package_descriptor_align(void) {
  return _Alignof(rinzu_native_package_descriptor_v1_t);
}
size_t rinzu_c_native_invocation_context_size(void) {
  return sizeof(rinzu_native_invocation_context_v1_t);
}
size_t rinzu_c_native_invocation_context_align(void) {
  return _Alignof(rinzu_native_invocation_context_v1_t);
}

_Static_assert(sizeof(rinzu_source_import_v2_t) == 48, "source import v2 size");
_Static_assert(_Alignof(rinzu_source_import_v2_t) == 8, "source import v2 align");
_Static_assert(sizeof(rinzu_source_package_descriptor_v2_t) == 40, "source descriptor v2 size");
_Static_assert(_Alignof(rinzu_source_package_descriptor_v2_t) == 8, "source descriptor v2 align");
size_t rinzu_c_source_v2_layout(size_t index) {
  const size_t values[] = {sizeof(rinzu_source_import_v2_t),
                           _Alignof(rinzu_source_import_v2_t),
                           offsetof(rinzu_source_import_v2_t, import_key),
                           offsetof(rinzu_source_import_v2_t, symbol),
                           offsetof(rinzu_source_import_v2_t, address),
                           sizeof(rinzu_source_package_descriptor_v2_t),
                           _Alignof(rinzu_source_package_descriptor_v2_t),
                           offsetof(rinzu_source_package_descriptor_v2_t, struct_size),
                           offsetof(rinzu_source_package_descriptor_v2_t, major),
                           offsetof(rinzu_source_package_descriptor_v2_t, minor),
                           offsetof(rinzu_source_package_descriptor_v2_t, descriptor),
                           offsetof(rinzu_source_package_descriptor_v2_t, descriptor_len),
                           offsetof(rinzu_source_package_descriptor_v2_t, import_count),
                           offsetof(rinzu_source_package_descriptor_v2_t, imports)};
  return index < sizeof(values) / sizeof(values[0]) ? values[index] : (size_t)-1;
}
