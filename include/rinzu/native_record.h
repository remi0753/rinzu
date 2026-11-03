#ifndef RINZU_NATIVE_RECORD_H
#define RINZU_NATIVE_RECORD_H
#include "rinzu/managed_composite.h"

typedef struct {
  unsigned kind;
  size_t offset;
} rinzu_native_record_field_t;
typedef struct {
  size_t type, size, alignment, count;
  const rinzu_native_record_field_t *fields;
} rinzu_native_record_layout_t;
typedef struct {
  unsigned char *pointer;
} rinzu_native_record_buffer_t;
typedef struct {
  uint64_t identity;
  const rinzu_native_record_layout_t *layout;
} rinzu_native_record_argument_t;
typedef struct {
  const rinzu_native_record_layout_t *layout;
  rinzu_native_record_buffer_t buffer;
  rinzu_managed_slot_t *staged;
} rinzu_native_record_entry_t;
typedef struct {
  rinzu_managed_record_loan_t link;
  rinzu_managed_data_t *owner;
  rinzu_managed_roots_t roots;
  rinzu_managed_root_set_t root_set;
  rinzu_native_record_entry_t *entries;
  size_t *arguments;
  size_t argument_count;
  unsigned phase;
} rinzu_native_record_loan_t;
enum { RINZU_RECORD_PREPARED, RINZU_RECORD_ENTERED, RINZU_RECORD_RETURNED, RINZU_RECORD_COMMITTED };
static inline int rinzu_native_record_same_layout(const rinzu_native_record_layout_t *left,
                                                  const rinzu_native_record_layout_t *right) {
  if (left == right) return left != NULL;
  if (!left || !right) return 0;
  if (left->type != right->type || left->size != right->size ||
      left->alignment != right->alignment || left->count != right->count)
    return 0;
  for (size_t field = 0; field < left->count; ++field)
    if (left->fields[field].kind != right->fields[field].kind ||
        left->fields[field].offset != right->fields[field].offset)
      return 0;
  return 1;
}
enum { RINZU_NATIVE_RECORD_OK, RINZU_NATIVE_NULL_RECORD, RINZU_NATIVE_RECORD_CONVERSION };

/* Consume target-Clang offsets; never infer C layout from managed slots. */
static inline void rinzu_native_record_validate(rinzu_managed_data_t *data,
                                                const rinzu_native_record_layout_t *layout) {
  rinzu_managed_native_owner(data);
  if (!layout || !data->schema || layout->type >= data->schema->count || !layout->size ||
      layout->size > PTRDIFF_MAX || !layout->alignment ||
      (layout->alignment & (layout->alignment - 1)) || layout->size % layout->alignment ||
      !layout->count || !layout->fields)
    rinzu_interop_fatal();
  const rinzu_value_type_t *type = &data->schema->types[layout->type];
  if ((type->kind != RINZU_VALUE_RECORD && type->kind != RINZU_VALUE_STATE) ||
      type->field_count != layout->count || !type->fields)
    rinzu_interop_fatal();
  size_t end = 0;
  for (size_t index = 0; index < layout->count; ++index) {
    rinzu_native_record_field_t field = layout->fields[index];
    size_t width = rinzu_scalar_width(field.kind) / 8;
    if (!width || layout->alignment < width || field.offset % width || field.offset < end ||
        field.offset > layout->size || width > layout->size - field.offset ||
        type->fields[index] >= data->schema->count)
      rinzu_interop_fatal();
    const rinzu_value_type_t *child = &data->schema->types[type->fields[index]];
    if (child->kind != RINZU_VALUE_SCALAR || child->scalar_kind != field.kind)
      rinzu_interop_fatal();
    end = field.offset + width;
  }
}
static inline uint64_t rinzu_native_record_read(const unsigned char *pointer,
                                                rinzu_native_record_field_t field) {
  uint64_t bits = 0;
  size_t width = rinzu_scalar_width(field.kind) / 8;
  switch (width) {
  case 1: {
    uint8_t value;
    memcpy(&value, pointer + field.offset, 1);
    bits = value;
    break;
  }
  case 2: {
    uint16_t value;
    memcpy(&value, pointer + field.offset, 2);
    bits = value;
    break;
  }
  case 4: {
    uint32_t value;
    memcpy(&value, pointer + field.offset, 4);
    bits = value;
    break;
  }
  case 8:
    memcpy(&bits, pointer + field.offset, 8);
    break;
  default:
    rinzu_interop_fatal();
  }
  if (field.kind < 4 && width < 8) {
    unsigned shift = (unsigned)(64 - width * 8);
    bits = (uint64_t)((int64_t)(bits << shift) >> shift);
  }
  return bits;
}
static inline void rinzu_native_record_write(unsigned char *pointer,
                                             rinzu_native_record_field_t field, uint64_t bits) {
  switch (rinzu_scalar_width(field.kind) / 8) {
  case 1: {
    uint8_t value = (uint8_t)bits;
    memcpy(pointer + field.offset, &value, 1);
    break;
  }
  case 2: {
    uint16_t value = (uint16_t)bits;
    memcpy(pointer + field.offset, &value, 2);
    break;
  }
  case 4: {
    uint32_t value = (uint32_t)bits;
    memcpy(pointer + field.offset, &value, 4);
    break;
  }
  case 8:
    memcpy(pointer + field.offset, &bits, 8);
    break;
  default:
    rinzu_interop_fatal();
  }
}
static inline rinzu_native_record_buffer_t
rinzu_managed_copy_record(rinzu_managed_data_t *data, const rinzu_native_record_layout_t *layout,
                          uint64_t identity) {
  rinzu_native_record_validate(data, layout);
  for (size_t index = 0; index < layout->count; ++index) {
    rinzu_managed_slot_t slot;
    if (rinzu_managed_project(data, identity, layout->type, -1, index, &slot) || slot.reference)
      rinzu_interop_fatal();
  }
  void *pointer = NULL;
  size_t alignment = layout->alignment;
  if (alignment < sizeof(void *))
    alignment = sizeof(void *);
  if (posix_memalign(&pointer, alignment, layout->size) || !pointer)
    rinzu_resource_fatal();
  memset(pointer, 0, layout->size);
  for (size_t index = 0; index < layout->count; ++index) {
    rinzu_managed_slot_t slot;
    if (rinzu_managed_project(data, identity, layout->type, -1, index, &slot))
      rinzu_interop_fatal();
    rinzu_native_record_write(pointer, layout->fields[index], slot.bits);
  }
  return (rinzu_native_record_buffer_t){pointer};
}
static inline void rinzu_native_record_buffer_dispose(rinzu_native_record_buffer_t *buffer) {
  if (!buffer)
    rinzu_interop_fatal();
  free(buffer->pointer);
  *buffer = (rinzu_native_record_buffer_t){0};
}
static inline int rinzu_managed_receive_record(rinzu_managed_data_t *data,
                                               const rinzu_native_record_layout_t *layout,
                                               const unsigned char *pointer, uint64_t *output) {
  rinzu_native_record_validate(data, layout);
  if (!output)
    rinzu_interop_fatal();
  if (!pointer)
    return RINZU_NATIVE_NULL_RECORD;
  for (rinzu_managed_record_loan_t *link = data->native_record_loans; link; link = link->next) {
    rinzu_native_record_loan_t *loan = (rinzu_native_record_loan_t *)link;
    if (loan->owner != data)
      rinzu_interop_fatal();
    for (size_t index = 0; index < link->count; ++index) {
      const rinzu_native_record_entry_t *entry = &loan->entries[index];
      if (entry->buffer.pointer == pointer && rinzu_native_record_same_layout(entry->layout, layout)) {
          *output = link->identities[index];
          return RINZU_NATIVE_RECORD_OK;
      }
    }
  }
  if (layout->count > SIZE_MAX / sizeof(rinzu_managed_slot_t))
    rinzu_resource_fatal();
  rinzu_managed_slot_t *slots = rinzu_failure_allocate(layout->count * sizeof(*slots));
  for (size_t index = 0; index < layout->count; ++index) {
    uint64_t bits = rinzu_native_record_read(pointer, layout->fields[index]);
    if (layout->fields[index].kind == RINZU_SCALAR_BOOL && bits > 1) {
      free(slots);
      return RINZU_NATIVE_RECORD_CONVERSION;
    }
    slots[index] = (rinzu_managed_slot_t){bits, 0};
  }
  if (rinzu_managed_composite(data, layout->type, -1, slots, layout->count, output))
    rinzu_interop_fatal();
  free(slots);
  return RINZU_NATIVE_RECORD_OK;
}

static inline rinzu_managed_value_t *rinzu_native_record_target(
    rinzu_managed_data_t *data, const rinzu_native_record_layout_t *layout, uint64_t identity) {
  rinzu_native_record_validate(data, layout);
  rinzu_managed_value_t *value = NULL;
  if (data->schema->types[layout->type].kind != RINZU_VALUE_STATE ||
      rinzu_managed_composite_resolve_owned(data, layout->type, identity, &value) ||
      !value || value->has_variant || value->count != layout->count)
    rinzu_interop_fatal();
  for (size_t field = 0; field < layout->count; ++field)
    if (value->slots[field].reference ||
        !rinzu_transfer_valid(layout->fields[field].kind, value->slots[field].bits))
      rinzu_interop_fatal();
  return value;
}
#define RINZU_NATIVE_RECORD_SCRATCH_LIMIT ((size_t)64 * 1024)
/* One allocation owns batch, alias map, staging and all distinct C structs.
   Common managed root frames keep their existing separately measured cache. */
static inline size_t rinzu_native_record_region(size_t *bytes, size_t count,
                                                size_t width, size_t alignment) {
  size_t padding = (alignment - *bytes % alignment) % alignment;
  if (padding > SIZE_MAX - *bytes || count > (SIZE_MAX - *bytes - padding) / width)
    rinzu_resource_fatal();
  size_t offset = *bytes + padding;
  *bytes = offset + count * width;
  return offset;
}
static inline rinzu_native_record_loan_t *rinzu_native_record_scratch(
    rinzu_managed_data_t *data, size_t bytes, size_t alignment) {
  rinzu_managed_record_loan_t **cursor = &data->native_record_pool;
  while (*cursor && ((*cursor)->storage_bytes < bytes ||
                     (*cursor)->storage_alignment < alignment))
    cursor = &(*cursor)->next;
  rinzu_native_record_loan_t *loan;
  if (*cursor) {
    rinzu_managed_record_loan_t *cached = *cursor;
    *cursor = cached->next;
    if (data->native_record_pool_bytes < cached->storage_bytes)
      rinzu_interop_fatal();
    data->native_record_pool_bytes -= cached->storage_bytes;
    cached->next = NULL;
    loan = (rinzu_native_record_loan_t *)cached;
  } else {
    void *pointer = NULL;
    if (alignment < sizeof(void *)) alignment = sizeof(void *);
    if (posix_memalign(&pointer, alignment, bytes) || !pointer)
      rinzu_resource_fatal();
    loan = pointer;
    memset(loan, 0, sizeof(*loan));
    loan->link.storage_bytes = bytes;
    loan->link.storage_alignment = alignment;
    if (data->native_record_allocations == SIZE_MAX) rinzu_resource_fatal();
    ++data->native_record_allocations;
  }
  if (!loan->link.storage_accounted &&
      atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
    rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, loan->link.storage_bytes);
    loan->link.storage_accounted = 1;
  }
  return loan;
}
/* Check the complete set before scratch allocation, roots or Busy state. */
static inline int rinzu_native_record_prepare(rinzu_managed_data_t *data,
    const rinzu_native_record_argument_t *arguments, size_t count,
    rinzu_native_record_loan_t **output) {
  rinzu_managed_native_owner(data);
  if (!arguments || !count || !output) rinzu_interop_fatal();
  *output = NULL;
  size_t distinct = 0, fields = 0, payload = 0, alignment = _Alignof(rinzu_native_record_loan_t);
  for (size_t index = 0; index < count; ++index) {
    const rinzu_native_record_layout_t *layout = arguments[index].layout;
    size_t previous = 0;
    while (previous < index && arguments[previous].identity != arguments[index].identity) ++previous;
    if (previous < index) {
      /* The same target was fully checked before any allocation or reentry.
         A different contract must still prove the complete matching layout. */
      if (arguments[previous].layout != layout) rinzu_native_record_validate(data, layout);
      if (!rinzu_native_record_same_layout(arguments[previous].layout, layout)) rinzu_interop_fatal();
    } else {
      rinzu_native_record_target(data, layout, arguments[index].identity);
      if (rinzu_managed_record_busy(data, arguments[index].identity)) return RINZU_VALUE_BUSY;
      if (distinct == SIZE_MAX || fields > SIZE_MAX - layout->count) rinzu_resource_fatal();
      ++distinct; fields += layout->count;
      (void)rinzu_native_record_region(&payload, 1, layout->size, layout->alignment);
      if (alignment < layout->alignment) alignment = layout->alignment;
    }
  }
  size_t bytes = sizeof(rinzu_native_record_loan_t);
  size_t entries_offset = rinzu_native_record_region(&bytes, distinct, sizeof(rinzu_native_record_entry_t), _Alignof(rinzu_native_record_entry_t));
  size_t arguments_offset = rinzu_native_record_region(&bytes, count, sizeof(size_t), _Alignof(size_t));
  size_t staged_offset = rinzu_native_record_region(&bytes, fields, sizeof(rinzu_managed_slot_t), _Alignof(rinzu_managed_slot_t));
  size_t payload_offset = rinzu_native_record_region(&bytes, 1, payload, alignment);
  rinzu_native_record_loan_t *loan = rinzu_native_record_scratch(data, bytes, alignment);
  loan->owner = data; loan->argument_count = count;
  loan->entries = (void *)((unsigned char *)loan + entries_offset);
  loan->arguments = (void *)((unsigned char *)loan + arguments_offset);
  rinzu_managed_slot_t *staged = (void *)((unsigned char *)loan + staged_offset);
  unsigned char *storage = (unsigned char *)loan + payload_offset;
  rinzu_managed_attach_roots(data, &loan->root_set, &loan->roots);
  rinzu_managed_enter(&loan->roots);
  uint64_t *identities = rinzu_managed_reserve_roots(&loan->roots, distinct);
  size_t used = 0;
  for (size_t argument = 0; argument < count; ++argument) {
    size_t index = 0;
    while (index < loan->link.count && identities[index] != arguments[argument].identity) ++index;
    loan->arguments[argument] = index;
    if (index < loan->link.count) continue;
    identities[index] = arguments[argument].identity;
    rinzu_native_record_entry_t *entry = &loan->entries[index];
    const rinzu_native_record_layout_t *layout = arguments[argument].layout;
    entry->layout = layout; entry->staged = staged; staged += layout->count;
    size_t offset = rinzu_native_record_region(&used, 1, layout->size, layout->alignment);
    entry->buffer.pointer = storage + offset;
    memset(entry->buffer.pointer, 0, layout->size);
    /* Preflight has checked this complete target. Scratch/root reservation
       uses system allocation only: no managed collection, entry or mutation
       can intervene. Resolve the stable identity instead of checking it twice. */
    rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, identities[index]);
    if (!value) rinzu_interop_fatal();
    for (size_t field = 0; field < layout->count; ++field)
      rinzu_native_record_write(entry->buffer.pointer, layout->fields[field], value->slots[field].bits);
    ++loan->link.count;
  }
  if (loan->link.count != distinct || used != payload) rinzu_interop_fatal();
  rinzu_managed_set_count(&loan->roots, distinct);
  loan->link.identities = identities; loan->link.busy = 1;
  loan->link.next = data->native_record_loans; data->native_record_loans = &loan->link;
  *output = loan; return RINZU_NATIVE_RECORD_OK;
}
static inline void rinzu_native_record_loan_owner(rinzu_managed_data_t *data,
                                                  rinzu_native_record_loan_t *loan) {
  rinzu_managed_native_owner(data);
  if (!loan)
    rinzu_interop_fatal();
  for (rinzu_managed_record_loan_t *link = data->native_record_loans; link; link = link->next)
    if (link == &loan->link && loan->owner == data)
      return;
  rinzu_interop_fatal();
}
static inline unsigned char *rinzu_native_record_argument_pointer(rinzu_managed_data_t *data,
    rinzu_native_record_loan_t *loan, size_t argument) {
  rinzu_native_record_loan_owner(data, loan);
  if (argument >= loan->argument_count)
    rinzu_interop_fatal();
  return loan->entries[loan->arguments[argument]].buffer.pointer;
}
static inline void rinzu_native_record_enter(rinzu_managed_data_t *data, rinzu_native_record_loan_t *loan) {
  rinzu_native_record_loan_owner(data, loan);
  if (loan->phase != RINZU_RECORD_PREPARED)
    rinzu_interop_fatal();
  loan->phase = RINZU_RECORD_ENTERED;
}
static inline void rinzu_native_record_returned(rinzu_managed_data_t *data, rinzu_native_record_loan_t *loan) {
  rinzu_native_record_loan_owner(data, loan);
  if (loan->phase != RINZU_RECORD_ENTERED)
    rinzu_interop_fatal();
  loan->phase = RINZU_RECORD_RETURNED;
}
/* Caller has settled every accepted callback. Stage every output before any
   publication; no allocation, collection or managed reentry occurs here. */
static inline void rinzu_native_record_commit(rinzu_managed_data_t *data, rinzu_native_record_loan_t *loan) {
  rinzu_native_record_loan_owner(data, loan);
  if (loan->phase != RINZU_RECORD_RETURNED)
    rinzu_interop_fatal();
  for (size_t index = 0; index < loan->link.count; ++index) {
    rinzu_native_record_entry_t *entry = &loan->entries[index];
    rinzu_native_record_target(data, entry->layout, loan->link.identities[index]);
    for (size_t field = 0; field < entry->layout->count; ++field) {
      uint64_t bits = rinzu_native_record_read(entry->buffer.pointer, entry->layout->fields[field]);
      if (!rinzu_transfer_valid(entry->layout->fields[field].kind, bits))
        rinzu_interop_fatal();
      entry->staged[field] = (rinzu_managed_slot_t){bits, 0};
    }
  }
  for (size_t index = 0; index < loan->link.count; ++index) {
    rinzu_native_record_entry_t *entry = &loan->entries[index];
    rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, loan->link.identities[index]);
    memcpy(value->slots, entry->staged, entry->layout->count * sizeof(*entry->staged));
  }
  loan->phase = RINZU_RECORD_COMMITTED;
  loan->link.busy = 0;
}
static inline void rinzu_native_record_release(rinzu_managed_data_t *data,
                                               rinzu_native_record_loan_t *loan) {
  rinzu_native_record_loan_owner(data, loan);
  if (loan->phase != RINZU_RECORD_PREPARED && loan->phase != RINZU_RECORD_COMMITTED)
    rinzu_interop_fatal();
  rinzu_managed_record_loan_t **link = &data->native_record_loans;
  while (*link != &loan->link)
    link = &(*link)->next;
  *link = loan->link.next;
  rinzu_managed_leave(&loan->roots);
  rinzu_managed_detach_roots(data, &loan->root_set);
  rinzu_managed_dispose(&loan->roots);
  size_t bytes = loan->link.storage_bytes, alignment = loan->link.storage_alignment;
  unsigned accounted = loan->link.storage_accounted;
  if (bytes <= RINZU_NATIVE_RECORD_SCRATCH_LIMIT &&
      data->native_record_pool_bytes <= RINZU_NATIVE_RECORD_SCRATCH_LIMIT - bytes) {
    /* No managed reference or root-set pointer remains live in cached storage.
       C payload and staging contain ABI scalars only, never Rinzu references. */
    memset(loan, 0, sizeof(*loan));
    loan->link.storage_bytes = bytes; loan->link.storage_alignment = alignment;
    loan->link.storage_accounted = accounted;
    loan->link.next = data->native_record_pool; data->native_record_pool = &loan->link;
    data->native_record_pool_bytes += bytes;
  } else {
    if (accounted) rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
    free(loan);
  }
}
#endif
