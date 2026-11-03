#ifndef RINZU_TYPED_TRANSPORT_H
#define RINZU_TYPED_TRANSPORT_H
#include "rinzu/value.h"

/* The closed signature supplies root types. Only exact-width scalar bits,
   lengths, nominal identities, tags and value contents enter the wire. */
enum {
  RINZU_TYPED_OK,
  RINZU_TYPED_SCHEMA,
  RINZU_TYPED_VALUE,
  RINZU_TYPED_LENGTH,
  RINZU_TYPED_TRAILING
};
typedef struct rinzu_typed_allocation {
  void *memory;
  size_t size;
  struct rinzu_typed_allocation *next;
} rinzu_typed_allocation_t;
typedef struct {
  rinzu_value_t *values;
  size_t count, native_allocations, native_bytes;
  rinzu_typed_allocation_t *allocations;
} rinzu_typed_packet_t;
static inline void rinzu_typed_packet_dispose(rinzu_typed_packet_t *packet) {
  while (packet->allocations) {
    rinzu_typed_allocation_t *item = packet->allocations;
    packet->allocations = item->next;
    if (!packet->native_allocations || item->size > packet->native_bytes)
      rinzu_failure_fatal();
    packet->native_allocations -= 1;
    packet->native_bytes -= item->size;
    free(item->memory);
    free(item);
  }
  if (packet->native_allocations || packet->native_bytes)
    rinzu_failure_fatal();
  *packet = (rinzu_typed_packet_t){0};
}
static inline void *rinzu_typed_allocate(rinzu_typed_packet_t *packet, size_t count, size_t width) {
  if (!count)
    return NULL;
  if (count > SIZE_MAX / width || count * width > SIZE_MAX - packet->native_bytes ||
      packet->native_allocations == SIZE_MAX)
    rinzu_failure_fatal();
  rinzu_typed_allocation_t *item = rinzu_failure_allocate(sizeof(*item));
  item->size = count * width;
  item->memory = rinzu_failure_allocate(item->size);
  item->next = packet->allocations;
  packet->allocations = item;
  packet->native_allocations += 1;
  packet->native_bytes += item->size;
  return item->memory;
}
static inline int rinzu_typed_schema(const rinzu_value_schema_t *schema, const size_t *roots,
                                     size_t count) {
  if (!schema || (count && !roots) || (schema->count && (!schema->types || !schema->transferable)))
    return RINZU_TYPED_SCHEMA;
  /* Transferable closes over every variant and excludes cycles/Other.
     Error metadata is transferable while intentionally lacking Equality. */
  for (size_t index = 0; index < count; ++index)
    if (roots[index] >= schema->count || !schema->transferable[roots[index]])
      return RINZU_TYPED_SCHEMA;
  return RINZU_TYPED_OK;
}
static inline int rinzu_typed_validate(const rinzu_value_schema_t *schema, const size_t *roots,
                                       size_t count, const rinzu_value_t *values) {
  int status = rinzu_typed_schema(schema, roots, count);
  if (status)
    return status;
  if (count && !values)
    return RINZU_TYPED_VALUE;
  for (size_t index = 0; index < count; ++index)
    if (rinzu_value_validate(schema, roots[index], &values[index]))
      return RINZU_TYPED_VALUE;
  return RINZU_TYPED_OK;
}
typedef struct {
  unsigned char *bytes;
  size_t offset, capacity;
} rinzu_typed_writer_t;
static inline int rinzu_typed_write(rinzu_typed_writer_t *writer, const unsigned char *bytes,
                                    size_t count) {
  if (count > SIZE_MAX - writer->offset ||
      (writer->bytes && count > writer->capacity - writer->offset))
    return RINZU_TYPED_LENGTH;
  if (writer->bytes && count)
    memcpy(writer->bytes + writer->offset, bytes, count);
  writer->offset += count;
  return RINZU_TYPED_OK;
}
static inline int rinzu_typed_write_integer(rinzu_typed_writer_t *writer, uint64_t bits,
                                            size_t count) {
  unsigned char bytes[8];
  for (size_t index = 0; index < count; ++index)
    bytes[index] = (unsigned char)(bits >> (index * 8));
  return rinzu_typed_write(writer, bytes, count);
}
static inline int rinzu_typed_walk(const rinzu_value_schema_t *schema, const size_t *roots,
                                   size_t count, const rinzu_value_t *values,
                                   rinzu_typed_writer_t *writer) {
  rinzu_value_frame_t *stack =
      count ? rinzu_failure_allocate(schema->count * sizeof(*stack)) : NULL;
  int status = RINZU_TYPED_OK;
  for (size_t root = 0; root < count && !status; ++root) {
    size_t depth = 1;
    stack[0] = (rinzu_value_frame_t){roots[root], 0, &values[root]};
    while (depth && !status) {
      rinzu_value_frame_t *frame = &stack[depth - 1];
      const rinzu_value_type_t *type = &schema->types[frame->type];
      const rinzu_value_t *value = frame->value;
      if (!frame->field) {
        if (type->kind == RINZU_VALUE_SCALAR)
          status = rinzu_typed_write_integer(writer, value->bits,
                                             rinzu_scalar_width(type->scalar_kind) / 8);
        else if (type->kind == RINZU_VALUE_STRING || type->kind == RINZU_VALUE_BYTES ||
                 (type->kind == RINZU_VALUE_DYNAMIC_BYTES || type->kind == RINZU_VALUE_ERROR)) {
          status = rinzu_typed_write_integer(writer, value->length, 8);
          if (!status)
            status = rinzu_typed_write(writer, value->bytes, value->length);
        } else if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_ENUM) {
          size_t length = strlen(type->identity);
          status = rinzu_typed_write_integer(writer, length, 8);
          if (!status)
            status = rinzu_typed_write(writer, (const unsigned char *)type->identity, length);
          if (!status && type->kind == RINZU_VALUE_ENUM)
            status = rinzu_typed_write_integer(writer, value->variant, 4);
        } else if (type->kind == RINZU_VALUE_LIST || type->kind == RINZU_VALUE_MAP)
          status = rinzu_typed_write_integer(writer, value->count, 8);
      }
      if (status)
        break;
      if (type->kind <= RINZU_VALUE_BYTES ||
          (type->kind == RINZU_VALUE_DYNAMIC_BYTES || type->kind == RINZU_VALUE_ERROR)) {
        depth -= 1;
        continue;
      }
      size_t fields = type->kind == RINZU_VALUE_MAP ? value->count * 2 : value->count;
      if (frame->field == fields) {
        depth -= 1;
        continue;
      }
      size_t field = frame->field++, child;
      const rinzu_value_t *next;
      if (type->kind == RINZU_VALUE_MAP) {
        child = field & 1 ? type->value : type->key;
        next = field & 1 ? &value->entries[field / 2].value : &value->entries[field / 2].key;
      } else {
        child =
            type->kind == RINZU_VALUE_LIST
                ? type->element
                : (type->kind == RINZU_VALUE_RECORD ? type->fields[field]
                                                    : type->variants[value->variant].types[field]);
        next = &value->fields[field];
      }
      if (depth == schema->count)
        rinzu_failure_fatal();
      stack[depth++] = (rinzu_value_frame_t){child, 0, next};
    }
  }
  free(stack);
  return status;
}
static inline int rinzu_typed_size(const rinzu_value_schema_t *schema, const size_t *roots,
                                   size_t count, const rinzu_value_t *values, size_t *destination) {
  if (!destination)
    return RINZU_TYPED_LENGTH;
  int status = rinzu_typed_validate(schema, roots, count, values);
  if (status)
    return status;
  rinzu_typed_writer_t writer = {0};
  status = rinzu_typed_walk(schema, roots, count, values, &writer);
  if (!status)
    *destination = writer.offset;
  return status;
}
static inline int rinzu_typed_encode(const rinzu_value_schema_t *schema, const size_t *roots,
                                     size_t count, const rinzu_value_t *values,
                                     unsigned char *bytes, size_t length) {
  size_t size = 0;
  int status = rinzu_typed_size(schema, roots, count, values, &size);
  if (status)
    return status;
  if (size != length || (length && !bytes))
    return RINZU_TYPED_LENGTH;
  rinzu_typed_writer_t writer = {bytes, 0, length};
  return rinzu_typed_walk(schema, roots, count, values, &writer);
}
typedef struct {
  const unsigned char *bytes;
  size_t length, offset;
} rinzu_typed_reader_t;
static inline int rinzu_typed_read(rinzu_typed_reader_t *reader, size_t count,
                                   const unsigned char **destination) {
  if (count > reader->length - reader->offset)
    return RINZU_TYPED_LENGTH;
  *destination = count ? reader->bytes + reader->offset : NULL;
  reader->offset += count;
  return RINZU_TYPED_OK;
}
static inline int rinzu_typed_read_integer(rinzu_typed_reader_t *reader, size_t width,
                                           uint64_t *destination) {
  const unsigned char *bytes;
  int status = rinzu_typed_read(reader, width, &bytes);
  if (status)
    return status;
  uint64_t bits = 0;
  for (size_t index = 0; index < width; ++index)
    bits |= (uint64_t)bytes[index] << (index * 8);
  *destination = bits;
  return RINZU_TYPED_OK;
}
/* Conservative positive lower bounds reject impossible count headers before
   any count-sized allocation. Unit alone contributes no wire bytes. */
static inline size_t rinzu_typed_minimum(const rinzu_value_type_t *type) {
  if (type->kind == RINZU_VALUE_UNIT)
    return 0;
  if (type->kind == RINZU_VALUE_SCALAR)
    return rinzu_scalar_width(type->scalar_kind) / 8;
  if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_ENUM)
    return (type->kind == RINZU_VALUE_RECORD ? 8 : 12) + strlen(type->identity);
  return 8;
}
typedef struct {
  size_t type, field;
  unsigned entered;
  rinzu_value_t *value;
} rinzu_typed_decode_frame_t;
static inline int rinzu_typed_decode(const rinzu_value_schema_t *schema, const size_t *roots,
                                     size_t count, const unsigned char *bytes, size_t length,
                                     rinzu_typed_packet_t *destination) {
  if (!destination || destination->values || destination->count || destination->allocations ||
      destination->native_allocations || destination->native_bytes || (length && !bytes))
    return RINZU_TYPED_LENGTH;
  int status = rinzu_typed_schema(schema, roots, count);
  if (status)
    return status;
  if (count > SIZE_MAX / sizeof(rinzu_value_t) ||
      schema->count > SIZE_MAX / sizeof(rinzu_typed_decode_frame_t))
    return RINZU_TYPED_LENGTH;
  rinzu_typed_packet_t packet = {0};
  packet.count = count;
  packet.values = rinzu_typed_allocate(&packet, count, sizeof(*packet.values));
  rinzu_typed_decode_frame_t *stack =
      count ? rinzu_failure_allocate(schema->count * sizeof(*stack)) : NULL;
  rinzu_typed_reader_t reader = {bytes, length, 0};
  for (size_t root = 0; root < count && !status; ++root) {
    size_t depth = 1;
    stack[0] = (rinzu_typed_decode_frame_t){roots[root], 0, 0, &packet.values[root]};
    while (depth && !status) {
      rinzu_typed_decode_frame_t *frame = &stack[depth - 1];
      const rinzu_value_type_t *type = &schema->types[frame->type];
      rinzu_value_t *value = frame->value;
      if (!frame->entered) {
        frame->entered = 1;
        value->kind = type->kind;
        if (type->kind == RINZU_VALUE_SCALAR) {
          value->scalar_kind = type->scalar_kind;
          status = rinzu_typed_read_integer(&reader, rinzu_scalar_width(type->scalar_kind) / 8,
                                            &value->bits);
          value->bits = rinzu_transfer_canonical(type->scalar_kind, value->bits);
          if (!status && !rinzu_transfer_valid(type->scalar_kind, value->bits))
            status = RINZU_TYPED_VALUE;
        } else if (type->kind == RINZU_VALUE_STRING || type->kind == RINZU_VALUE_BYTES ||
                   (type->kind == RINZU_VALUE_DYNAMIC_BYTES || type->kind == RINZU_VALUE_ERROR)) {
          uint64_t size = 0;
          const unsigned char *payload = NULL;
          status = rinzu_typed_read_integer(&reader, 8, &size);
          if (!status && size > SIZE_MAX)
            status = RINZU_TYPED_LENGTH;
          if (!status && type->kind == RINZU_VALUE_BYTES && size != type->bytes_length)
            status = RINZU_TYPED_VALUE;
          if (!status &&
              (type->kind == RINZU_VALUE_DYNAMIC_BYTES || type->kind == RINZU_VALUE_ERROR) &&
              size > INT64_MAX)
            status = RINZU_TYPED_LENGTH;
          if (!status)
            status = rinzu_typed_read(&reader, (size_t)size, &payload);
          if (!status && type->kind == RINZU_VALUE_ERROR &&
              !rinzu_error_value_valid(payload, (size_t)size))
            status = RINZU_TYPED_VALUE;
          if (!status && type->kind == RINZU_VALUE_STRING &&
              !rinzu_utf8_valid(payload, (size_t)size))
            status = RINZU_TYPED_VALUE;
          if (!status) {
            value->length = (size_t)size;
            unsigned char *copy = rinzu_typed_allocate(&packet, value->length, 1);
            if (value->length)
              memcpy(copy, payload, value->length);
            value->bytes = copy;
          }
        } else if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_ENUM) {
          uint64_t size = 0;
          const unsigned char *identity = NULL;
          status = rinzu_typed_read_integer(&reader, 8, &size);
          if (!status && size != strlen(type->identity))
            status = RINZU_TYPED_VALUE;
          if (!status)
            status = rinzu_typed_read(&reader, (size_t)size, &identity);
          if (!status && memcmp(identity, type->identity, (size_t)size) != 0)
            status = RINZU_TYPED_VALUE;
          value->identity = type->identity;
          if (!status && type->kind == RINZU_VALUE_ENUM) {
            uint64_t variant = 0;
            status = rinzu_typed_read_integer(&reader, 4, &variant);
            value->variant = (uint32_t)variant;
            if (!status && variant >= type->variant_count)
              status = RINZU_TYPED_VALUE;
          }
          if (!status)
            value->count = type->kind == RINZU_VALUE_RECORD ? type->field_count
                                                            : type->variants[value->variant].count;
        } else if (type->kind == RINZU_VALUE_LIST || type->kind == RINZU_VALUE_MAP) {
          uint64_t size = 0;
          status = rinzu_typed_read_integer(&reader, 8, &size);
          size_t width =
              type->kind == RINZU_VALUE_MAP ? sizeof(rinzu_value_pair_t) : sizeof(rinzu_value_t);
          size_t minimum = type->kind == RINZU_VALUE_MAP
                               ? rinzu_typed_minimum(&schema->types[type->key]) +
                                     rinzu_typed_minimum(&schema->types[type->value])
                               : rinzu_typed_minimum(&schema->types[type->element]);
          if (!status && (size > SIZE_MAX / width ||
                          (minimum && size > (reader.length - reader.offset) / minimum)))
            status = RINZU_TYPED_LENGTH;
          if (!status)
            value->count = (size_t)size;
        }
        if (!status && type->kind >= RINZU_VALUE_RECORD) {
          if (type->kind == RINZU_VALUE_MAP)
            value->entries = rinzu_typed_allocate(&packet, value->count, sizeof(*value->entries));
          else
            value->fields = rinzu_typed_allocate(&packet, value->count, sizeof(*value->fields));
        }
      }
      if (status)
        break;
      if (type->kind <= RINZU_VALUE_BYTES ||
          (type->kind == RINZU_VALUE_DYNAMIC_BYTES || type->kind == RINZU_VALUE_ERROR)) {
        depth -= 1;
        continue;
      }
      size_t fields = type->kind == RINZU_VALUE_MAP ? value->count * 2 : value->count;
      if (frame->field == fields) {
        depth -= 1;
        continue;
      }
      size_t field = frame->field++, child;
      rinzu_value_t *next;
      if (type->kind == RINZU_VALUE_MAP) {
        child = field & 1 ? type->value : type->key;
        rinzu_value_pair_t *entries = (rinzu_value_pair_t *)value->entries;
        next = field & 1 ? &entries[field / 2].value : &entries[field / 2].key;
      } else {
        child =
            type->kind == RINZU_VALUE_LIST
                ? type->element
                : (type->kind == RINZU_VALUE_RECORD ? type->fields[field]
                                                    : type->variants[value->variant].types[field]);
        next = &((rinzu_value_t *)value->fields)[field];
      }
      if (depth == schema->count)
        rinzu_failure_fatal();
      stack[depth++] = (rinzu_typed_decode_frame_t){child, 0, 0, next};
    }
  }
  free(stack);
  if (!status && reader.offset != length)
    status = RINZU_TYPED_TRAILING;
  if (!status)
    status = rinzu_typed_validate(schema, roots, count, packet.values);
  if (status)
    rinzu_typed_packet_dispose(&packet);
  else
    *destination = packet;
  return status;
}
#endif
