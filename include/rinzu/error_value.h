#ifndef RINZU_ERROR_VALUE_H
#define RINZU_ERROR_VALUE_H
#include "rinzu/key.h"
#include "rinzu/failure.h"
#include <string.h>

/* Acyclic metadata table. Every reference points to an earlier node. */
typedef struct {
  const unsigned char *bytes;
  size_t length, offset;
} rinzu_error_cursor_t;
typedef struct {
  int64_t code;
  const unsigned char *message, *context, *cleanup;
  size_t message_length, context_length, begin, end;
  rinzu_source_position_t source;
  int64_t native_status;
  unsigned native_present;
  uint32_t cause, cleanup_count;
} rinzu_error_node_t;
typedef struct {
  unsigned char *bytes;
  size_t length, capacity;
} rinzu_error_buffer_t;
static inline int rinzu_error_read(rinzu_error_cursor_t *c, size_t n, const unsigned char **out) {
  if (n > c->length - c->offset)
    return 1;
  *out = c->bytes + c->offset;
  c->offset += n;
  return 0;
}
static inline uint64_t rinzu_error_integer(const unsigned char *bytes, size_t width) {
  uint64_t result = 0;
  for (size_t i = 0; i < width; i++)
    result |= (uint64_t)bytes[i] << (8 * i);
  return result;
}
static inline int rinzu_error_number(rinzu_error_cursor_t *c, size_t width, uint64_t *value) {
  const unsigned char *bytes;
  if (rinzu_error_read(c, width, &bytes))
    return 1;
  *value = rinzu_error_integer(bytes, width);
  return 0;
}
static inline int rinzu_error_node_read(rinzu_error_cursor_t *c, uint32_t index,
                                        rinzu_error_node_t *n) {
  uint64_t value;
  const unsigned char *bytes;
  *n = (rinzu_error_node_t){0};
  n->begin = c->offset;
  if (rinzu_error_number(c, 8, &value))
    return 1;
  n->code = (int64_t)value;
  if (rinzu_error_number(c, 8, &value) || value > SIZE_MAX ||
      rinzu_error_read(c, (size_t)value, &n->message))
    return 1;
  n->message_length = (size_t)value;
  if (rinzu_error_number(c, 8, &value) || value > SIZE_MAX ||
      rinzu_error_read(c, (size_t)value, &n->context))
    return 1;
  n->context_length = (size_t)value;
  if (!rinzu_utf8_valid(n->message, n->message_length) ||
      !rinzu_utf8_valid(n->context, n->context_length))
    return 1;
  if (rinzu_error_number(c, 1, &value) || value > 1)
    return 1;
  n->source.present = (unsigned)value;
  if (rinzu_error_number(c, 8, &n->source.file))
    return 1;
  if (rinzu_error_number(c, 4, &value))
    return 1;
  n->source.start_byte = (uint32_t)value;
  if (rinzu_error_number(c, 4, &value))
    return 1;
  n->source.end_byte = (uint32_t)value;
  if (!n->source.present && (n->source.file || n->source.start_byte || n->source.end_byte))
    return 1;
  if (rinzu_error_number(c, 1, &value) || value > 1)
    return 1;
  n->native_present = (unsigned)value;
  if (rinzu_error_number(c, 8, &value))
    return 1;
  n->native_status = (int64_t)value;
  if (!n->native_present && n->native_status)
    return 1;
  if (rinzu_error_number(c, 4, &value))
    return 1;
  n->cause = (uint32_t)value;
  if (n->cause != UINT32_MAX && n->cause >= index)
    return 1;
  if (rinzu_error_number(c, 4, &value) || value > (c->length - c->offset) / 4)
    return 1;
  n->cleanup_count = (uint32_t)value;
  if (rinzu_error_read(c, 4 * (size_t)n->cleanup_count, &bytes))
    return 1;
  n->cleanup = bytes;
  for (uint32_t i = 0; i < n->cleanup_count; i++)
    if (rinzu_error_integer(bytes + 4 * (size_t)i, 4) >= index)
      return 1;
  n->end = c->offset;
  return 0;
}
static inline int rinzu_error_value_valid(const unsigned char *bytes, size_t length) {
  if (!bytes || length < 4)
    return 0;
  uint32_t count = (uint32_t)rinzu_error_integer(bytes, 4);
  if (!count || count > (length - 4) / 58)
    return 0;
  rinzu_error_cursor_t c = {bytes, length, 4};
  rinzu_error_node_t node;
  for (uint32_t i = 0; i < count; i++)
    if (rinzu_error_node_read(&c, i, &node))
      return 0;
  return c.offset == length;
}
static inline rinzu_error_node_t rinzu_error_node(const unsigned char *bytes, size_t length,
                                                  uint32_t index) {
  if (length < 4 || index >= rinzu_error_integer(bytes, 4))
    rinzu_failure_fatal();
  rinzu_error_cursor_t c = {bytes, length, 4};
  rinzu_error_node_t node;
  for (uint32_t i = 0; i <= index; i++)
    if (rinzu_error_node_read(&c, i, &node))
      rinzu_failure_fatal();
  return node;
}
static inline void rinzu_error_append(rinzu_error_buffer_t *b, const unsigned char *bytes,
                                      size_t length) {
  if (b->length == SIZE_MAX || length > SIZE_MAX - b->length - 1)
    rinzu_failure_fatal();
  size_t required = b->length + length + 1;
  if (required > b->capacity) {
    size_t capacity = b->capacity ? b->capacity : 64;
    while (capacity < required) {
      if (capacity > SIZE_MAX / 2) {
        capacity = required;
        break;
      }
      capacity *= 2;
    }
    unsigned char *next = rinzu_failure_allocate(capacity);
    if (b->length)
      memcpy(next, b->bytes, b->length);
    /* Input may be a slice of the old buffer. Keep it alive through both
       copies, including when a self-append forces capacity growth. */
    if (length)
      memcpy(next + b->length, bytes, length);
    free(b->bytes);
    b->bytes = next;
    b->capacity = capacity;
  } else if (length) {
    memmove(b->bytes + b->length, bytes, length);
  }
  b->length += length;
  b->bytes[b->length] = 0;
}
static inline void rinzu_error_write(rinzu_error_buffer_t *b, uint64_t value, size_t width) {
  unsigned char bytes[8];
  for (size_t i = 0; i < width; i++)
    bytes[i] = (unsigned char)(value >> (8 * i));
  rinzu_error_append(b, bytes, width);
}
static inline void rinzu_error_encode_node(rinzu_error_buffer_t *b, const rinzu_error_node_t *n,
                                           uint32_t offset) {
  rinzu_error_write(b, (uint64_t)n->code, 8);
  rinzu_error_write(b, n->message_length, 8);
  rinzu_error_append(b, n->message, n->message_length);
  rinzu_error_write(b, n->context_length, 8);
  rinzu_error_append(b, n->context, n->context_length);
  rinzu_error_write(b, n->source.present, 1);
  rinzu_error_write(b, n->source.file, 8);
  rinzu_error_write(b, n->source.start_byte, 4);
  rinzu_error_write(b, n->source.end_byte, 4);
  rinzu_error_write(b, n->native_present, 1);
  rinzu_error_write(b, (uint64_t)n->native_status, 8);
  rinzu_error_write(b, n->cause == UINT32_MAX ? UINT32_MAX : n->cause + offset, 4);
  rinzu_error_write(b, n->cleanup_count, 4);
  for (uint32_t i = 0; i < n->cleanup_count; i++)
    rinzu_error_write(b, rinzu_error_integer(n->cleanup + 4 * (size_t)i, 4) + offset, 4);
}
static inline rinzu_error_buffer_t rinzu_error_single(int64_t code, const unsigned char *message,
                                                      size_t length) {
  rinzu_error_buffer_t b = {0};
  rinzu_error_write(&b, 1, 4);
  rinzu_error_node_t n = {
      .code = code, .message = message, .message_length = length, .cause = UINT32_MAX};
  rinzu_error_encode_node(&b, &n, 0);
  return b;
}
static inline int64_t rinzu_error_code_from_failure(int64_t cause) {
  if (cause == 5)
    return 1;
  if (cause >= 8 && cause <= 14)
    return cause - 6;
  rinzu_failure_fatal();
  return 0;
}
#endif
