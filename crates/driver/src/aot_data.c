#include "rinzu/byte_scan.h"

static void rinzu_aot_throw_error(RinzuContext *, uint64_t);
/* String payloads are validated UTF-8. All indexes below count scalar values. */
static size_t rinzu_text_next(const rinzu_managed_value_t *text, size_t offset) {
  if (offset >= text->length)
    return text->length;
  offset++;
  while (offset < text->length && (text->bytes[offset] & 0xc0) == 0x80)
    offset++;
  return offset;
}
static int rinzu_text_offset(const rinzu_managed_value_t *text, int64_t index, size_t *output) {
  if (index < 0 || (uint64_t)index > text->scalar_length)
    return 0;
  if (text->scalar_length == text->length) {
    *output = (size_t)index;
    return 1;
  }
  size_t offset = 0;
  while (index && offset < text->length) {
    offset = rinzu_text_next(text, offset);
    index--;
  }
  *output = offset;
  return index == 0;
}
static size_t rinzu_text_match(const rinzu_managed_value_t *text,
                               const rinzu_managed_value_t *needle, size_t start) {
  if (!needle->length)
    return start;
  if (start > text->length || needle->length > text->length - start)
    return SIZE_MAX;
  size_t last = text->length - needle->length;
  /* A validated UTF-8 needle cannot begin with a continuation byte. Searching
     for its first byte therefore only finds scalar boundaries in valid text. */
  while (start <= last) {
    const unsigned char *found = memchr(text->bytes + start, needle->bytes[0], last - start + 1);
    if (!found)
      break;
    size_t offset = (size_t)(found - text->bytes);
    if (!memcmp(found, needle->bytes, needle->length))
      return offset;
    start = offset + 1;
  }
  return SIZE_MAX;
}
static int64_t rinzu_data_failure(RinzuContext *context, int64_t code, const char *message,
                                  const char *name) {
  rinzu_error_buffer_t blob = {0};
  rinzu_error_write(&blob, 1, 4);
  rinzu_error_node_t node = {.code = code,
                             .message = (const unsigned char *)message,
                             .message_length = strlen(message),
                             .context = (const unsigned char *)name,
                             .context_length = strlen(name),
                             .cause = UINT32_MAX};
  rinzu_error_encode_node(&blob, &node, 0);
  rinzu_managed_data_t *data = rinzu_context_data(context);
  uint64_t error = rinzu_managed_rich_error(data, blob.bytes, blob.length);
  free(blob.bytes);
  context->allocations++;
  rinzu_aot_throw_error(context, error);
  return 0;
}
static int64_t rinzu_aot_record_busy(RinzuContext *context) {
  return rinzu_data_failure(context, 4, "native record is already in use", "");
}
static int64_t rinzu_text_operation(RinzuContext *context, rinzu_managed_data_t *data,
                                    int64_t operation, size_t result, const uint64_t *bits,
                                    rinzu_managed_value_t *const *resolved) {
  uint64_t output = 0;
  if (operation == RINZU_DATA_FORMAT_INT) {
    char buffer[32];
    int length = snprintf(buffer, sizeof buffer, "%lld", (long long)(int64_t)bits[0]);
    if (length < 0 || (size_t)length >= sizeof buffer)
      rinzu_failure_fatal();
    output = rinzu_managed_string_copy_owned(data, (const unsigned char *)buffer, (size_t)length,
                                             (size_t)length);
  } else {
    rinzu_managed_value_t *text = resolved[0];
    if (!text)
      rinzu_failure_fatal();
    if (operation == RINZU_DATA_UTF8_DECODE) {
      if (text->kind != RINZU_MANAGED_BYTES)
        rinzu_failure_fatal();
      size_t scalars = rinzu_utf8_scalar_length(text->bytes, text->length);
      if (scalars == SIZE_MAX)
        return rinzu_data_failure(context, 200, "invalid UTF-8", "utf8Decode");
      output = rinzu_managed_string_copy_owned(data, text->bytes, text->length, scalars);
    } else {
      if (text->kind != RINZU_MANAGED_STRING || text->scalar_length > INT64_MAX)
        rinzu_failure_fatal();
      if (operation == RINZU_DATA_STRING_LENGTH) {
        return (int64_t)text->scalar_length;
      }
      if (operation == RINZU_DATA_UTF8_ENCODE) {
        if (rinzu_managed_bytes(data, text->length, text->bytes, text->length, &output))
          rinzu_failure_fatal();
      } else if (operation == RINZU_DATA_PARSE_INT) {
        size_t begin = text->length && (text->bytes[0] == '+' || text->bytes[0] == '-') ? 1 : 0;
        if (begin == text->length)
          return rinzu_data_failure(context, 201, "invalid decimal integer", "parseInt");
        for (size_t i = begin; i < text->length; i++)
          if (text->bytes[i] < '0' || text->bytes[i] > '9')
            return rinzu_data_failure(context, 201, "invalid decimal integer", "parseInt");
        int negative = text->bytes[0] == '-';
        uint64_t limit = negative ? UINT64_C(1) << 63 : INT64_MAX, magnitude = 0;
        for (size_t i = begin; i < text->length; i++) {
          unsigned digit = (unsigned)(text->bytes[i] - '0');
          if (magnitude > (limit - digit) / 10)
            return rinzu_data_failure(context, 202, "integer overflow", "parseInt");
          magnitude = magnitude * 10 + digit;
        }
        return (int64_t)(negative ? UINT64_C(0) - magnitude : magnitude);
      } else if (operation == RINZU_DATA_STRING_SPLIT) {
        rinzu_managed_value_t *needle = resolved[1];
        if (!needle || needle->kind != RINZU_MANAGED_STRING || needle->scalar_length == SIZE_MAX)
          rinzu_failure_fatal();
        size_t count = 0;
        if (!needle->length) {
          for (size_t offset = 0; offset < text->length; offset = rinzu_text_next(text, offset))
            count++;
        } else {
          count = 1;
          size_t offset = 0, found;
          while ((found = rinzu_text_match(text, needle, offset)) != SIZE_MAX) {
            count++;
            offset = found + needle->length;
          }
        }
        if (count > SIZE_MAX / sizeof(rinzu_managed_slot_t))
          rinzu_failure_fatal();
        rinzu_managed_slot_t *slots = count ? rinzu_failure_allocate(count * sizeof(*slots)) : NULL;
        size_t offset = 0;
        for (size_t i = 0; i < count; i++) {
          size_t end = !needle->length ? rinzu_text_next(text, offset)
                                       : rinzu_text_match(text, needle, offset);
          if (end == SIZE_MAX)
            end = text->length;
          slots[i] = (rinzu_managed_slot_t){
              rinzu_managed_string(data, text->bytes ? text->bytes + offset : NULL, end - offset),
              1};
          context->allocations++;
          offset = end + needle->length;
        }
        if (rinzu_managed_composite_owned(data, result, -1, slots, count, &output))
          rinzu_failure_fatal();
        free(slots);
      } else {
        rinzu_managed_slot_t slot = {0};
        int valid = 0;
        size_t begin = 0, end = 0;
        if (operation == RINZU_DATA_STRING_FIND) {
          rinzu_managed_value_t *needle = resolved[1];
          if (!needle || needle->kind != RINZU_MANAGED_STRING || needle->scalar_length == SIZE_MAX)
            rinzu_failure_fatal();
          valid = rinzu_text_offset(text, (int64_t)bits[2], &begin);
          size_t found = valid ? rinzu_text_match(text, needle, begin) : SIZE_MAX;
          valid = found != SIZE_MAX;
          if (valid) {
            if (text->scalar_length == text->length)
              slot.bits = found;
            else {
              slot.bits = bits[2];
              while (begin < found) {
                begin = rinzu_text_next(text, begin);
                slot.bits++;
              }
            }
          }
        } else if (operation == RINZU_DATA_STRING_GET || operation == RINZU_DATA_STRING_SLICE) {
          valid = rinzu_text_offset(text, (int64_t)bits[1], &begin);
          if (operation == RINZU_DATA_STRING_GET) {
            valid = valid && begin < text->length;
            end = rinzu_text_next(text, begin);
          } else {
            valid = valid && (int64_t)bits[2] >= (int64_t)bits[1] && bits[2] <= text->scalar_length;
            if (valid) {
              end = begin;
              if (text->scalar_length == text->length)
                end = (size_t)bits[2];
              else
                for (uint64_t scalar = bits[1]; scalar < bits[2]; scalar++)
                  end = rinzu_text_next(text, end);
            }
          }
          if (valid) {
            if (begin == 0 && end == text->length)
              slot = (rinzu_managed_slot_t){text->identity, 1};
            else {
              size_t scalars = operation == RINZU_DATA_STRING_GET ? 1 : bits[2] - bits[1];
              slot = (rinzu_managed_slot_t){
                  rinzu_managed_string_copy_owned(data, text->bytes + begin, end - begin, scalars),
                  1};
              context->allocations++;
            }
          }
        } else
          rinzu_failure_fatal();
        if (rinzu_managed_composite_owned(data, result, valid, &slot, (size_t)valid, &output))
          rinzu_failure_fatal();
      }
    }
  }
  context->allocations++;
  return (int64_t)output;
}

static int64_t rinzu_numeric_operation(RinzuContext *context, int64_t operation,
                                       const uint64_t *types, const uint64_t *bits) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  unsigned kind = data->schema->types[types[0]].scalar_kind;
  if (operation == RINZU_DATA_MATH_ABS || operation == RINZU_DATA_MATH_SQRT ||
      operation == RINZU_DATA_FLOAT_IS_NAN) {
    if (kind == RINZU_SCALAR_F32) {
      float n = rinzu_scalar_f32(bits[0]);
      if (operation == RINZU_DATA_FLOAT_IS_NAN)
        return isnan(n) != 0;
      return (int64_t)rinzu_scalar_f32_bits(operation == RINZU_DATA_MATH_ABS ? fabsf(n) : sqrtf(n));
    }
    if (kind == RINZU_SCALAR_F64) {
      double n = rinzu_scalar_f64(bits[0]);
      if (operation == RINZU_DATA_FLOAT_IS_NAN)
        return isnan(n) != 0;
      return (int64_t)rinzu_scalar_f64_bits(operation == RINZU_DATA_MATH_ABS ? fabs(n) : sqrt(n));
    }
    rinzu_failure_fatal();
  }
  if (kind >= RINZU_SCALAR_F32)
    rinzu_failure_fatal();
  unsigned width = rinzu_scalar_width(kind);
  uint64_t mask = UINT64_MAX >> (64 - width), a = bits[0], b = 0, value = 0;

  if (operation != RINZU_DATA_BIT_NOT)
    b = bits[1];
  switch (operation) {
  case RINZU_DATA_BIT_AND:
    value = a & b;
    break;
  case RINZU_DATA_BIT_OR:
    value = a | b;
    break;
  case RINZU_DATA_BIT_XOR:
    value = a ^ b;
    break;
  case RINZU_DATA_BIT_NOT:
    value = ~a;
    break;
  case RINZU_DATA_WRAPPING_ADD:
    value = a + b;
    break;
  case RINZU_DATA_WRAPPING_SUB:
    value = a - b;
    break;
  case RINZU_DATA_WRAPPING_MUL:
    value = a * b;
    break;
  case RINZU_DATA_SHIFT_LEFT:
  case RINZU_DATA_SHIFT_RIGHT:
    if ((int64_t)b < 0 || b >= width)
      return rinzu_data_failure(context, 301, "invalid shift count",
                                operation == RINZU_DATA_SHIFT_LEFT ? "shiftLeft" : "shiftRight");
    if (operation == RINZU_DATA_SHIFT_LEFT)
      value = a << b;
    else {
      value = a >> b;
      if (b && rinzu_scalar_signed(kind) && (a & (UINT64_C(1) << (width - 1))))
        value |= UINT64_MAX << (64 - b);
    }
    break;
  case RINZU_DATA_CHECKED_ADD:
  case RINZU_DATA_CHECKED_SUB:
  case RINZU_DATA_CHECKED_MUL: {
    unsigned arithmetic = (unsigned)(operation - RINZU_DATA_CHECKED_ADD) + 1;
    rinzu_scalar_result_t calculated = rinzu_scalar_arithmetic(kind, arithmetic, a, b);
    if (calculated.status) {
      if (calculated.status != 1)
        rinzu_failure_fatal();
      const char *name = operation == RINZU_DATA_CHECKED_ADD   ? "checkedAdd"
                         : operation == RINZU_DATA_CHECKED_SUB ? "checkedSub"
                                                               : "checkedMul";
      return rinzu_data_failure(context, 300, "integer overflow", name);
    }
    value = calculated.bits;
    break;
  }
  default:
    rinzu_failure_fatal();
  }
  value &= mask;
  if (rinzu_scalar_signed(kind) && width < 64 && (value & (UINT64_C(1) << (width - 1))))
    value |= ~mask;
  return (int64_t)value;
}

static uint64_t rinzu_error_prefix(rinzu_managed_data_t *data, const unsigned char *bytes,
                                   size_t length, uint32_t index) {
  rinzu_error_node_t node = rinzu_error_node(bytes, length, index);
  rinzu_error_buffer_t copy = {0};
  rinzu_error_write(&copy, (uint64_t)index + 1, 4);
  rinzu_error_append(&copy, bytes + 4, node.end - 4);
  uint64_t result = rinzu_managed_rich_error(data, copy.bytes, copy.length);
  free(copy.bytes);
  return result;
}
static int64_t rinzu_error_operation(RinzuContext *context, int64_t operation, size_t result,
                                     const uint64_t *bits) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  uint64_t output = 0;
  rinzu_error_buffer_t blob = {0};
  if (operation == RINZU_DATA_ERROR_CREATE || operation == RINZU_DATA_NATIVE_STATUS) {
    rinzu_managed_value_t *text = rinzu_managed_string_value(data, bits[1]);
    int64_t code = (int64_t)bits[0];
    if (operation == RINZU_DATA_NATIVE_STATUS && code == 0) {
      return 0;
    } else {
      if (operation == RINZU_DATA_ERROR_CREATE)
        blob = rinzu_error_single(code, text->bytes, text->length);
      else {
        rinzu_error_write(&blob, 1, 4);
        static const unsigned char message[] = "native operation failed";
        rinzu_error_node_t n = {.code = 100,
                                .message = message,
                                .message_length = sizeof(message) - 1,
                                .context = text->bytes,
                                .context_length = text->length,
                                .native_present = 1,
                                .native_status = code,
                                .cause = UINT32_MAX};
        rinzu_error_encode_node(&blob, &n, 0);
      }
      output = rinzu_managed_rich_error(data, blob.bytes, blob.length);
      if (operation == RINZU_DATA_NATIVE_STATUS) {
        free(blob.bytes);
        context->allocations++;
        rinzu_aot_throw_error(context, output);
        return 0;
      }
    }
  } else {
    rinzu_managed_value_t *error = rinzu_managed_find(data, bits[0]);
    blob = rinzu_managed_error_blob(data, error);
    uint32_t count = (uint32_t)rinzu_error_integer(blob.bytes, 4);
    rinzu_error_node_t root = rinzu_error_node(blob.bytes, blob.length, count - 1);
    if (operation == RINZU_DATA_ERROR_CODE) {
      free(blob.bytes);
      return root.code;
    }
    if (operation == RINZU_DATA_ERROR_MESSAGE || operation == RINZU_DATA_ERROR_CONTEXT) {
      output = rinzu_managed_string(
          data, operation == RINZU_DATA_ERROR_MESSAGE ? root.message : root.context,
          operation == RINZU_DATA_ERROR_MESSAGE ? root.message_length : root.context_length);
    } else if (operation == RINZU_DATA_ERROR_WITH_CONTEXT) {
      rinzu_managed_value_t *text = rinzu_managed_string_value(data, bits[1]);
      root.context = text->bytes;
      root.context_length = text->length;
      rinzu_error_buffer_t copy = {0};
      rinzu_error_append(&copy, blob.bytes, root.begin);
      rinzu_error_encode_node(&copy, &root, 0);
      output = rinzu_managed_rich_error(data, copy.bytes, copy.length);
      free(copy.bytes);
    } else if (operation == RINZU_DATA_ERROR_WITH_CAUSE) {
      rinzu_error_buffer_t cause =
          rinzu_managed_error_blob(data, rinzu_managed_find(data, bits[1]));
      uint32_t other = (uint32_t)rinzu_error_integer(cause.bytes, 4);
      int variant = 0;
      rinzu_error_buffer_t copy = {0};
      if (count > 64 || other > 64 - count) {
        static const unsigned char message[] = "Error cause graph exceeds 64 nodes";
        copy = rinzu_error_single(101, message, sizeof(message) - 1);
        variant = 1;
      } else {
        rinzu_error_write(&copy, count + other, 4);
        rinzu_error_append(&copy, blob.bytes + 4, root.begin - 4);
        rinzu_error_cursor_t cursor = {cause.bytes, cause.length, 4};
        for (uint32_t i = 0; i < other; i++) {
          rinzu_error_node_t n;
          if (rinzu_error_node_read(&cursor, i, &n))
            rinzu_failure_fatal();
          rinzu_error_encode_node(&copy, &n, count - 1);
        }
        root.cause = count + other - 2;
        rinzu_error_encode_node(&copy, &root, 0);
      }
      output = rinzu_managed_rich_error(data, copy.bytes, copy.length);
      free(copy.bytes);
      free(cause.bytes);
      if (variant) {
        free(blob.bytes);
        context->allocations++;
        rinzu_aot_throw_error(context, output);
        return 0;
      }
    } else if (operation == RINZU_DATA_ERROR_CLEANUP) {
      if ((size_t)root.cleanup_count > SIZE_MAX / sizeof(rinzu_managed_slot_t))
        rinzu_failure_fatal();
      rinzu_managed_slot_t *slots =
          root.cleanup_count ? rinzu_failure_allocate((size_t)root.cleanup_count * sizeof(*slots))
                             : NULL;
      for (uint32_t i = 0; i < root.cleanup_count; i++) {
        slots[i] = (rinzu_managed_slot_t){
            rinzu_error_prefix(data, blob.bytes, blob.length,
                               (uint32_t)rinzu_error_integer(root.cleanup + 4 * (size_t)i, 4)),
            1};
        context->allocations++;
      }
      if (rinzu_managed_composite(data, result, -1, slots, root.cleanup_count, &output))
        rinzu_failure_fatal();
      free(slots);
    } else {
      rinzu_managed_slot_t child = {0};
      int present = 0;
      if (operation == RINZU_DATA_ERROR_CAUSE) {
        present = root.cause != UINT32_MAX;
        if (present) {
          child = (rinzu_managed_slot_t){
              rinzu_error_prefix(data, blob.bytes, blob.length, root.cause), 1};
          context->allocations++;
        }
      } else if (operation == RINZU_DATA_ERROR_NATIVE_STATUS) {
        present = (int)root.native_present;
        child.bits = (uint64_t)root.native_status;
      } else if (operation == RINZU_DATA_ERROR_SOURCE_FILE) {
        present = (int)root.source.present;
        child.bits = root.source.file;
      } else if (operation == RINZU_DATA_ERROR_SOURCE_START) {
        present = (int)root.source.present;
        child.bits = root.source.start_byte;
      } else if (operation == RINZU_DATA_ERROR_SOURCE_END) {
        present = (int)root.source.present;
        child.bits = root.source.end_byte;
      } else
        rinzu_failure_fatal();
      if (rinzu_managed_composite(data, result, present, &child, (size_t)present, &output))
        rinzu_failure_fatal();
    }
  }
  free(blob.bytes);
  context->allocations++;
  return (int64_t)output;
}

/* The operation entry has checked the Owner and every typed argument. Keep
   those resolutions local: this path cannot yield, reenter native code or GC. */
/* Keep bulk traversal separate from the established byte operations. */
static __attribute__((noinline)) int64_t
rinzu_bytes_concat_all_cached(RinzuContext *context, rinzu_managed_data_t *data, size_t element,
                              rinzu_managed_value_t *input) {
  enum { RINZU_CONCAT_SAVED_PARTS = 256 };
  rinzu_managed_value_t *parts[RINZU_CONCAT_SAVED_PARTS];
  size_t total = 0, nonempty = 0, tail = 0;
  rinzu_managed_value_t *shared = NULL;
  for (size_t index = 0; index < input->count; ++index) {
    rinzu_managed_value_t *part = NULL;
    if (rinzu_managed_slot_resolve_owned(data, element, input->slots[index], &part) || !part ||
        part->kind != RINZU_MANAGED_BYTES || part->length > (size_t)INT64_MAX - total)
      rinzu_failure_fatal();
    total += part->length;
    if (part->length) {
      shared = part;
      if (nonempty < RINZU_CONCAT_SAVED_PARTS) {
        parts[nonempty] = part;
        tail = index + 1;
      }
      ++nonempty;
    } else if (!shared)
      shared = part;
  }
  if (nonempty <= 1 && shared)
    return (int64_t)shared->identity;
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, total);
  size_t offset = 0;
  /* These borrowed pointers cannot move: allocation may resize the identity
     index, but cannot collect, yield or reenter. Only initialized, nonempty
     entries are read. Keep stack space bounded and no heap scratch is needed. */
  size_t saved = nonempty < RINZU_CONCAT_SAVED_PARTS ? nonempty : RINZU_CONCAT_SAVED_PARTS;
  for (size_t index = 0; index < saved; ++index) {
    rinzu_managed_value_t *part = parts[index];
    memcpy(value->bytes + offset, part->bytes, part->length);
    offset += part->length;
  }
  /* Large Lists reuse the saved prefix too; only the remaining suffix needs
     another handle lookup. All slots were validated before allocating. */
  if (nonempty > saved)
    for (size_t index = tail; index < input->count; ++index) {
      rinzu_managed_value_t *part = rinzu_managed_lookup_owned(data, input->slots[index].bits);
      if (part->length)
        memcpy(value->bytes + offset, part->bytes, part->length);
      offset += part->length;
    }
  context->allocations++;
  return (int64_t)value->identity;
}

/* Empty and single-element Lists need no pointer scratch or stack canary.
   Validate the same closed-schema boundary before selecting either path. */
static __attribute__((noinline)) int64_t rinzu_bytes_concat_all(RinzuContext *context,
                                                                rinzu_managed_data_t *data,
                                                                size_t result,
                                                                const uint64_t *types,
                                                                rinzu_managed_value_t *input) {
  if (!input)
    rinzu_failure_fatal();
  const rinzu_value_type_t *layout = &data->schema->types[types[0]];
  if (input->kind != RINZU_MANAGED_COMPOSITE || layout->kind != RINZU_VALUE_LIST ||
      layout->element >= data->schema->count ||
      data->schema->types[layout->element].kind != RINZU_VALUE_DYNAMIC_BYTES ||
      data->schema->types[result].kind != RINZU_VALUE_DYNAMIC_BYTES)
    rinzu_failure_fatal();
  if (input->count > 1)
    return rinzu_bytes_concat_all_cached(context, data, layout->element, input);
  if (input->count) {
    rinzu_managed_value_t *part = NULL;
    if (rinzu_managed_slot_resolve_owned(data, layout->element, input->slots[0], &part) || !part ||
        part->kind != RINZU_MANAGED_BYTES || part->length > (size_t)INT64_MAX)
      rinzu_failure_fatal();
    return (int64_t)part->identity;
  }
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, 0);
  context->allocations++;
  return (int64_t)value->identity;
}

/* Bounds are checked by either the public operation or its closed compiler
   entry. This kernel owns the same immutable slice/copy and logical charge. */
static inline uint64_t rinzu_bytes_slice_value_owned(RinzuContext *context,
                                                     rinzu_managed_data_t *data,
                                                     const rinzu_managed_value_t *input,
                                                     uint64_t start, uint64_t end) {
  size_t n = (size_t)(end - start);
  if (n == input->length)
    return input->identity;
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, n);
  if (n)
    memcpy(value->bytes, input->bytes + start, n);
  context->allocations++;
  return value->identity;
}

static int64_t rinzu_byte_operation(RinzuContext *context, rinzu_managed_data_t *data,
                                    int64_t operation, size_t result, const uint64_t *types,
                                    const uint64_t *bits, rinzu_managed_value_t *const *resolved) {
  rinzu_managed_value_t *input = resolved[0];
  if (!input || (input->kind != RINZU_MANAGED_COMPOSITE && input->kind != RINZU_MANAGED_BYTES))
    rinzu_failure_fatal();
  size_t length = input->kind == RINZU_MANAGED_COMPOSITE ? input->count : input->length;
  if (length > INT64_MAX)
    rinzu_failure_fatal();
  if (operation == RINZU_DATA_BYTES_LENGTH)
    return (int64_t)length;
  if (input->kind == RINZU_MANAGED_COMPOSITE) {
    if (operation != RINZU_DATA_BYTE_ARRAY ||
        data->schema->types[types[0]].kind != RINZU_VALUE_LIST ||
        data->schema->types[result].kind != RINZU_VALUE_DYNAMIC_BYTES)
      rinzu_failure_fatal();
    rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
    unsigned char *destination = value->bytes;
    const rinzu_managed_slot_t *slots = input->slots;
    for (size_t i = 0; i < length; i++)
      destination[i] = (unsigned char)slots[i].bits;
    context->allocations++;
    return (int64_t)value->identity;
  }
  const unsigned char *source = input->bytes;
  if (operation == RINZU_DATA_BYTES_TRIM) {
    rinzu_managed_value_t *set = resolved[1];
    if (!set || set->kind != RINZU_MANAGED_BYTES ||
        data->schema->types[result].kind != RINZU_VALUE_DYNAMIC_BYTES)
      rinzu_failure_fatal();
    uint64_t members[4] = {0};
    for (size_t i = 0; i < set->length; ++i)
      members[set->bytes[i] >> 6] |= UINT64_C(1) << (set->bytes[i] & 63);
    size_t start = 0, end = length;
    while (start < end && (members[source[start] >> 6] & (UINT64_C(1) << (source[start] & 63))))
      start++;
    while (end > start && (members[source[end - 1] >> 6] & (UINT64_C(1) << (source[end - 1] & 63))))
      end--;
    if (!start && end == length)
      return (int64_t)input->identity;
    rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, end - start);
    if (end > start)
      memcpy(value->bytes, source + start, end - start);
    context->allocations++;
    return (int64_t)value->identity;
  }
  uint64_t output = 0;
  if (operation == RINZU_DATA_BYTES_FIND_ANY || operation == RINZU_DATA_BYTES_FIND_NOT_ANY ||
      operation == RINZU_DATA_BYTES_CONTAINS_ANY || operation == RINZU_DATA_BYTES_ALL_IN) {
    rinzu_managed_value_t *set = resolved[1];
    if (!set || set->kind != RINZU_MANAGED_BYTES)
      rinzu_failure_fatal();
    int predicate =
        operation == RINZU_DATA_BYTES_CONTAINS_ANY || operation == RINZU_DATA_BYTES_ALL_IN;
    int in_set =
        operation == RINZU_DATA_BYTES_FIND_ANY || operation == RINZU_DATA_BYTES_CONTAINS_ANY;
    size_t found = SIZE_MAX;
    if (predicate || ((int64_t)bits[2] >= 0 && bits[2] <= length))
      found = rinzu_byte_find_set(source, length, set->bytes, set->length,
                                  predicate ? 0 : (size_t)bits[2], in_set);
    if (predicate) {
      if (data->schema->types[result].kind != RINZU_VALUE_SCALAR ||
          data->schema->types[result].scalar_kind != RINZU_SCALAR_BOOL)
        rinzu_failure_fatal();
      return operation == RINZU_DATA_BYTES_CONTAINS_ANY ? found != SIZE_MAX : found == SIZE_MAX;
    }
    rinzu_managed_slot_t slot = {found == SIZE_MAX ? 0 : found, 0};
    if (rinzu_managed_composite_owned(data, result, found != SIZE_MAX, &slot, found != SIZE_MAX,
                                      &output))
      rinzu_failure_fatal();
  } else if (operation == RINZU_DATA_BYTES_LOWER_ASCII) {
    rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
    unsigned char *destination = value->bytes;
    for (size_t i = 0; i < length; ++i) {
      unsigned char byte = source[i];
      destination[i] = byte >= 'A' && byte <= 'Z' ? byte + ('a' - 'A') : byte;
    }
    output = value->identity;
  } else if (operation == RINZU_DATA_BYTES_GET || operation == RINZU_DATA_BYTES_SLICE) {
    int valid = (int64_t)bits[1] >= 0 && bits[1] <= length;
    if (operation == RINZU_DATA_BYTES_GET)
      valid = valid && bits[1] < length;
    else
      valid = valid && (int64_t)bits[2] >= 0 && bits[1] <= bits[2] && bits[2] <= length;
    rinzu_managed_slot_t slot = {0};
    if (valid) {
      if (operation == RINZU_DATA_BYTES_GET)
        slot.bits = source[bits[1]];
      else {
        slot.bits = rinzu_bytes_slice_value_owned(context, data, input, bits[1], bits[2]);
        slot.reference = 1;
      }
    }
    if (rinzu_managed_composite_owned(data, result, valid, &slot, (size_t)valid, &output))
      rinzu_failure_fatal();
  } else {
    if (operation == RINZU_DATA_BYTES_CONCAT) {
      rinzu_managed_value_t *other = resolved[1];
      if (!other || other->kind != RINZU_MANAGED_BYTES ||
          other->length > (size_t)INT64_MAX - length)
        rinzu_failure_fatal();
      /* All arguments were checked at entry. Immutable values have content
         equality; sharing inside this Domain exposes no writable address. */
      if (!length)
        return (int64_t)other->identity;
      if (!other->length)
        return (int64_t)input->identity;
      size_t n = length + other->length;
      rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, n);
      unsigned char *destination = value->bytes;
      if (length)
        memcpy(destination, source, length);
      if (other->length)
        memcpy(destination + length, other->bytes, other->length);
      output = value->identity;
    } else if (operation == RINZU_DATA_BYTES_XOR) {
      rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
      unsigned char *destination = value->bytes;
      for (size_t i = 0; i < length; ++i)
        destination[i] = source[i] ^ (unsigned char)bits[1];
      output = value->identity;
    } else if (operation == RINZU_DATA_BYTE_ARRAY) {
      if (data->schema->types[result].kind != RINZU_VALUE_DYNAMIC_BYTES)
        rinzu_failure_fatal();
      /* Fixed and dynamic bytes are the same immutable, owned payload. Entry
         validated the source kind/length/Owner; typed caller roots retain it. */
      return (int64_t)input->identity;
    } else if (operation != RINZU_DATA_BYTES_COPY)
      rinzu_failure_fatal();
    if (!output) {
      rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
      if (length)
        memcpy(value->bytes, source, length);
      output = value->identity;
    }
  }
  context->allocations++;
  return (int64_t)output;
}

/* The closed catalog supplies slot tags; numeric bits never imply references. */
static inline __attribute__((always_inline)) int64_t rinzu_aot_data_operation_owner_mode(
    void *raw, int64_t operation, int64_t result_type, const uint64_t *types, const uint64_t *bits,
    int64_t count, unsigned owner_proven) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (result_type < 0 || (uint64_t)result_type >= data->schema->count)
    rinzu_failure_fatal();
  if (!owner_proven)
    rinzu_managed_owner(data);
  if (operation == RINZU_DATA_PROCESS_ARGUMENTS && count == 0)
    return rinzu_aot_program_arguments(context, data, (size_t)result_type);
#ifdef RINZU_AOT_HAS_JSON
  if (operation == RINZU_DATA_JSON_NULL && count == 0)
    return rinzu_aot_json_operation(context, data, operation, (size_t)result_type, NULL, NULL,
                                    NULL);
#endif
  if (count < 1 || count > 3 || !types || !bits)
    rinzu_failure_fatal();
  size_t input = (size_t)types[0], result = (size_t)result_type;
  if (input >= data->schema->count)
    rinzu_failure_fatal();
  const rinzu_value_type_t *layout = &data->schema->types[input];
  rinzu_managed_value_t *resolved[3] = {0};
  for (size_t index = 0; index < (size_t)count; index++) {
    if (types[index] >= data->schema->count)
      rinzu_failure_fatal();
    if (rinzu_managed_slot_resolve_owned(
            data, (size_t)types[index],
            (rinzu_managed_slot_t){bits[index], rinzu_program_slot_references[types[index]]},
            &resolved[index]))
      rinzu_failure_fatal();
  }
#ifdef RINZU_AOT_HAS_JSON
  if (operation >= RINZU_DATA_JSON_PARSE && operation <= RINZU_DATA_JSON_FLOAT_VALUE)
    return rinzu_aot_json_operation(context, data, operation, result, types, bits, resolved);
#endif
  if (operation == RINZU_DATA_PANIC) {
    rinzu_managed_value_t *message = resolved[0];
    if (count != 1 || !message || message->kind != RINZU_MANAGED_STRING ||
        data->schema->types[result].kind != RINZU_VALUE_UNIT)
      rinzu_failure_fatal();
    /* Copy before entering native-safe state. Collection must never invalidate
       the bytes while stderr blocks or another Domain makes progress. */
    size_t length = message->length;
    unsigned char *bytes = rinzu_failure_allocate(length ? length : 1);
    if (length)
      memcpy(bytes, message->bytes, length);
    rinzu_failure_fail(&context->pending, 16, context->source);
    rinzu_owner_native_frame_t native = {0};
    rinzu_owner_native_enter(rinzu_context_transitions(context), &native, context->roots.depth,
                             rinzu_context_native_roots(context));
    rinzu_stdio_panic(bytes, length);
    rinzu_owner_native_leave(rinzu_context_transitions(context), &native);
    free(bytes);
    return 0;
  }
  if (operation == RINZU_DATA_BYTES_XOR || operation == RINZU_DATA_BYTES_FIND_ANY ||
      operation == RINZU_DATA_BYTES_FIND_NOT_ANY || operation == RINZU_DATA_BYTES_LOWER_ASCII ||
      operation == RINZU_DATA_BYTES_CONTAINS_ANY || operation == RINZU_DATA_BYTES_ALL_IN ||
      operation == RINZU_DATA_BYTES_TRIM || operation == RINZU_DATA_BYTES_CONCAT_ALL) {
    if (operation == RINZU_DATA_BYTES_CONCAT_ALL)
      return rinzu_bytes_concat_all(context, data, result, types, resolved[0]);
    return rinzu_byte_operation(context, data, operation, result, types, bits, resolved);
  }
  if (operation >= RINZU_DATA_LIST_CONCAT_ALL && operation <= RINZU_DATA_COLLECTION_FINISH)
    return rinzu_aot_collection_construction(context, data, operation, result, types, bits,
                                             (size_t)count, resolved[0]);
  if (operation >= RINZU_DATA_BIT_AND)
    return rinzu_numeric_operation(context, operation, types, bits);
  if (operation >= RINZU_DATA_STRING_LENGTH)
    return rinzu_text_operation(context, data, operation, result, bits, resolved);
  if (operation >= RINZU_DATA_ERROR_CREATE)
    return rinzu_error_operation(context, operation, result, bits);
  if (operation >= RINZU_DATA_BYTE_ARRAY)
    return rinzu_byte_operation(context, data, operation, result, types, bits, resolved);
  rinzu_managed_value_t *value = resolved[0];
  if (!value || value->kind != RINZU_MANAGED_COMPOSITE || value->type != input ||
      (layout->kind != RINZU_VALUE_LIST && layout->kind != RINZU_VALUE_MAP))
    rinzu_failure_fatal();
  size_t length = layout->kind == RINZU_VALUE_MAP ? value->count / 2 : value->count;
  if (operation == RINZU_DATA_LIST_LENGTH || operation == RINZU_DATA_MAP_LENGTH) {
    if (length > INT64_MAX)
      rinzu_failure_fatal();
    return (int64_t)length;
  }
  if (value->collection_building)
    rinzu_failure_fatal();
  if (operation == RINZU_DATA_MAP_GET || operation == RINZU_DATA_MAP_INSERT ||
      operation == RINZU_DATA_MAP_REMOVE) {
    rinzu_managed_slot_t wanted = {bits[1], rinzu_program_slot_references[types[1]]};
    uint64_t hash = 0, output = 0;
    size_t bucket = 0, selected = SIZE_MAX;
    if (rinzu_managed_value_hash(data, layout->key, wanted, &hash) ||
        rinzu_managed_map_probe_owned(data, layout->key, value->slots, value->map_index, wanted,
                                      hash, &bucket, &selected))
      rinzu_failure_fatal();
    if (operation == RINZU_DATA_MAP_GET) {
      rinzu_managed_slot_t child =
          selected == SIZE_MAX ? (rinzu_managed_slot_t){0} : value->slots[selected + 1];
      if (rinzu_managed_composite_owned(data, result, selected != SIZE_MAX, &child,
                                        selected != SIZE_MAX, &output))
        rinzu_failure_fatal();
    } else {
      if (operation == RINZU_DATA_MAP_INSERT && selected == SIZE_MAX &&
          !rinzu_collection_length_fits(length, 1))
        return rinzu_aot_collection_overflow(context, rinzu_map_length_overflow, length, 1);
      rinzu_managed_slot_t replacement =
          operation == RINZU_DATA_MAP_INSERT
              ? (rinzu_managed_slot_t){bits[2], rinzu_program_slot_references[types[2]]}
              : (rinzu_managed_slot_t){0};
      output = rinzu_managed_map_update_owned(data, value, selected, wanted, replacement, hash,
                                              operation == RINZU_DATA_MAP_REMOVE);
    }
    context->allocations++;
    return (int64_t)output;
  }
  if (operation == RINZU_DATA_LIST_APPEND || operation == RINZU_DATA_LIST_SET ||
      operation == RINZU_DATA_LIST_REMOVE) {
    int append = operation == RINZU_DATA_LIST_APPEND;
    int remove = operation == RINZU_DATA_LIST_REMOVE;
    /* IR verifies the exact arity. Do not keep count live past operand
       validation: that adds induction/comparison work to every operation. */
    if (layout->kind != RINZU_VALUE_LIST)
      rinzu_failure_fatal();
    if (append ? (result != input || types[1] != layout->element)
               : (data->schema->types[types[1]].kind != RINZU_VALUE_SCALAR ||
                  data->schema->types[types[1]].scalar_kind != RINZU_SCALAR_I64 ||
                  (!remove && types[2] != layout->element)))
      rinzu_failure_fatal();
    if (append && !rinzu_collection_length_fits(length, 1))
      return rinzu_aot_collection_overflow(context, rinzu_list_length_overflow, length, 1);
    size_t selected = SIZE_MAX;
    int found = !append && (int64_t)bits[1] >= 0 && bits[1] < length;
    if (found)
      selected = (size_t)bits[1];
    uint64_t output = 0;
    if (append || found) {
      size_t replacement_index = append ? 1 : 2;
      rinzu_managed_slot_t replacement =
          remove ? (rinzu_managed_slot_t){0}
                 : (rinzu_managed_slot_t){bits[replacement_index],
                                          rinzu_program_slot_references[types[replacement_index]]};
      output = rinzu_managed_list_update_owned(data, value, selected, replacement, remove);
      context->allocations++;
    }
    if (!append) {
      rinzu_managed_slot_t child = {output, 1};
      if (rinzu_managed_composite_owned(data, result, found, &child, (size_t)found, &output))
        rinzu_failure_fatal();
      context->allocations++;
    }
    return (int64_t)output;
  }
  if (value->count > SIZE_MAX / sizeof(rinzu_managed_slot_t) - 2)
    rinzu_failure_fatal();
  int reading = operation == RINZU_DATA_LIST_GET || operation == RINZU_DATA_MAP_GET;
  /* Reads inspect the owned immutable slots directly; only updates copy them. */
  rinzu_managed_slot_t *slots =
      reading ? value->slots : rinzu_failure_allocate((value->count + 2) * sizeof *slots);
  size_t used = value->count, selected = SIZE_MAX;
  if (used && !reading)
    memcpy(slots, value->slots, used * sizeof *slots);
  if (operation == RINZU_DATA_LIST_GET) {
    if ((int64_t)bits[1] >= 0 && bits[1] < length)
      selected = (size_t)bits[1];
  }
  int64_t variant = -1;
  uint64_t output = 0;
  if (operation == RINZU_DATA_LIST_GET || operation == RINZU_DATA_MAP_GET) {
    variant = selected != SIZE_MAX;
    rinzu_managed_slot_t child = {0};
    if (variant)
      child = slots[selected + (operation == RINZU_DATA_MAP_GET)];
    if (rinzu_managed_composite_owned(data, result, variant, &child, (size_t)variant, &output))
      rinzu_failure_fatal();
    context->allocations++;
    return (int64_t)output;
  } else if (operation == RINZU_DATA_MAP_KEYS) {
    for (size_t index = 0; index < length; index++)
      slots[index] = slots[2 * index];
    used = length;
  } else
    rinzu_failure_fatal();
  if (rinzu_managed_composite(data, result, variant, slots, used, &output))
    rinzu_failure_fatal();
  free(slots);
  context->allocations++;
  return (int64_t)output;
}
int64_t rinzu_baseline_data_operation(void *raw, int64_t operation, int64_t result_type,
                                      const uint64_t *types, const uint64_t *bits, int64_t count) {
  return rinzu_aot_data_operation_owner_mode(raw, operation, result_type, types, bits, count, 0);
}
int64_t rinzu_baseline_data_operation_owner_proven(void *raw, int64_t operation,
                                                   int64_t result_type, const uint64_t *types,
                                                   const uint64_t *bits, int64_t count) {
  return rinzu_aot_data_operation_owner_mode(raw, operation, result_type, types, bits, count, 1);
}

/* Closed, verified IR length signatures supply no dynamic type metadata.
   Keep the actual operand's Owner/Domain, kind and representable length checks.
   These helpers do not allocate, yield, reenter or change publication/poll order. */
int64_t rinzu_baseline_bytes_int_length(void *raw, int64_t bits) {
  rinzu_managed_value_t *value = rinzu_managed_find(rinzu_context_data(raw), (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_BYTES || value->length > INT64_MAX)
    rinzu_failure_fatal();
  return (int64_t)value->length;
}
int64_t rinzu_baseline_string_scalar_length(void *raw, int64_t bits) {
  rinzu_managed_value_t *value = rinzu_managed_find(rinzu_context_data(raw), (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_STRING || value->scalar_length > INT64_MAX)
    rinzu_failure_fatal();
  return (int64_t)value->scalar_length;
}
static inline __attribute__((always_inline)) int64_t rinzu_aot_collection_length_owner_mode(
    void *raw, int64_t bits, int64_t type, unsigned owner_proven) {
  rinzu_managed_data_t *data = rinzu_context_data(raw);
  if (!owner_proven)
    rinzu_managed_owner(data);
  if (type < 0 || (uint64_t)type >= data->schema->count)
    rinzu_failure_fatal();
  unsigned kind = data->schema->types[type].kind;
  if (kind != RINZU_VALUE_LIST && kind != RINZU_VALUE_MAP)
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, (uint64_t)bits);
  if (!value || value->kind != RINZU_MANAGED_COMPOSITE || value->type != (size_t)type ||
      value->has_variant || (kind == RINZU_VALUE_MAP && value->count % 2))
    rinzu_failure_fatal();
  size_t length = kind == RINZU_VALUE_MAP ? value->count / 2 : value->count;
  if (length > INT64_MAX)
    rinzu_failure_fatal();
  return (int64_t)length;
}
int64_t rinzu_baseline_collection_length(void *raw, int64_t bits, int64_t type) {
  return rinzu_aot_collection_length_owner_mode(raw, bits, type, 0);
}
int64_t rinzu_baseline_collection_length_owner_proven(void *raw, int64_t bits, int64_t type) {
  return rinzu_aot_collection_length_owner_mode(raw, bits, type, 1);
}

static size_t rinzu_data_type(int64_t type) {
  if (type < 0 || (uint64_t)type >= rinzu_program_schema.count)
    rinzu_failure_fatal();
  return (size_t)type;
}
static rinzu_managed_slot_t rinzu_data_slot(size_t type, uint64_t bits) {
  return (rinzu_managed_slot_t){bits, rinzu_program_slot_references[type]};
}
/* Only a verified leaf switch uses this representation. Both None (zero) and
   Some (a managed ByteArray identity) fit the existing reference root slots.
   The original logical Option charge is retained even though its physical
   object and its repeated tag/payload resolution are unnecessary. */
static inline __attribute__((always_inline)) int64_t
rinzu_aot_bytes_slice_payload_owner_mode(void *raw, int64_t result_type, int64_t bits,
                                         int64_t start, int64_t end, unsigned owner_proven) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  size_t result = rinzu_data_type(result_type);
  const rinzu_value_type_t *option = &data->schema->types[result];
  if (option->kind != RINZU_VALUE_ENUM || option->variant_count != 2 ||
      option->variants[0].count != 0 || option->variants[1].count != 1)
    rinzu_failure_fatal();
  size_t element = option->variants[1].types[0];
  if (element >= data->schema->count ||
      data->schema->types[element].kind != RINZU_VALUE_DYNAMIC_BYTES)
    rinzu_failure_fatal();
  if (!owner_proven)
    rinzu_managed_owner(data);
  rinzu_managed_value_t *input = rinzu_managed_lookup_owned(data, (uint64_t)bits);
  if (!input || input->kind != RINZU_MANAGED_BYTES || input->length > INT64_MAX)
    rinzu_failure_fatal();
  int valid = start >= 0 && end >= 0 && start <= end && (uint64_t)end <= input->length;
  uint64_t output =
      valid ? rinzu_bytes_slice_value_owned(context, data, input, (uint64_t)start, (uint64_t)end)
            : 0;
  context->allocations++;
  return (int64_t)output;
}
int64_t rinzu_baseline_bytes_slice_payload(void *raw, int64_t result_type, int64_t bits,
                                           int64_t start, int64_t end) {
  return rinzu_aot_bytes_slice_payload_owner_mode(raw, result_type, bits, start, end, 0);
}
int64_t rinzu_baseline_bytes_slice_payload_owner_proven(void *raw, int64_t result_type,
                                                        int64_t bits, int64_t start, int64_t end) {
  return rinzu_aot_bytes_slice_payload_owner_mode(raw, result_type, bits, start, end, 1);
}
static inline __attribute__((always_inline)) int64_t
rinzu_aot_construct_owner_mode(void *raw, int64_t type, int64_t variant, const uint64_t *bits,
                               int64_t count, unsigned owner_proven) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  size_t index = rinzu_data_type(type);
  if (count < 0 || (uint64_t)count > SIZE_MAX / sizeof(rinzu_managed_slot_t) || (count && !bits))
    rinzu_failure_fatal();
  const rinzu_value_type_t *layout = &rinzu_program_schema.types[index];
  if (rinzu_managed_layout(layout, variant, (size_t)count))
    rinzu_failure_fatal();
  if (context->max_allocations && context->allocations >= context->max_allocations) {
    rinzu_failure_fail(&context->pending, 3, context->source);
    return 0;
  }
  rinzu_managed_slot_t small[16];
  rinzu_managed_slot_t *slots =
      (uint64_t)count <= 16 ? small : rinzu_failure_allocate((size_t)count * sizeof *slots);
  for (size_t field = 0; field < (size_t)count; field++) {
    size_t child = rinzu_managed_child_type(layout, variant, field);
    slots[field] = rinzu_data_slot(child, bits[field]);
  }
  uint64_t result = 0;
  int status = owner_proven ? rinzu_managed_composite_impl_owned(data, index, variant, slots,
                                                                 (size_t)count, &result, 3)
                            : rinzu_managed_constructor_charged(data, index, variant, slots,
                                                                (size_t)count, &result);
  if (slots != small)
    free(slots);
  if (status)
    rinzu_failure_fatal();
  context->allocations += 1;
  return (int64_t)result;
}
int64_t rinzu_baseline_construct(void *raw, int64_t type, int64_t variant, const uint64_t *bits,
                                 int64_t count) {
  return rinzu_aot_construct_owner_mode(raw, type, variant, bits, count, 0);
}
int64_t rinzu_baseline_construct_owner_proven(void *raw, int64_t type, int64_t variant,
                                              const uint64_t *bits, int64_t count) {
  return rinzu_aot_construct_owner_mode(raw, type, variant, bits, count, 1);
}
int64_t rinzu_baseline_store_field(void *raw, int64_t bits, int64_t type, int64_t field,
                                   int64_t value) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (type < 0 || (uint64_t)type >= data->schema->count || field < 0)
    rinzu_failure_fatal();
  const rinzu_value_type_t *layout = &data->schema->types[type];
  if (layout->kind != RINZU_VALUE_STATE || (uint64_t)field >= layout->field_count)
    rinzu_failure_fatal();
  rinzu_managed_slot_t slot = rinzu_data_slot(layout->fields[field], (uint64_t)value);
  if (rinzu_managed_set_state_field(data, (uint64_t)bits, (size_t)type, (size_t)field, slot))
    rinzu_failure_fatal();
  return 0;
}

int64_t rinzu_baseline_project(void *raw, int64_t bits, int64_t type, int64_t variant,
                               int64_t field) {
  rinzu_managed_data_t *data = rinzu_context_data(raw);
  size_t index = rinzu_data_type(type);
  if (field < 0)
    rinzu_failure_fatal();
  rinzu_managed_slot_t result = {0};
  int status = rinzu_managed_project(data, (uint64_t)bits, index, variant, (size_t)field, &result);
  if (status == RINZU_VALUE_BUSY)
    return rinzu_aot_record_busy(raw);
  if (status)
    rinzu_failure_fatal();
  return (int64_t)result.bits;
}
int64_t rinzu_baseline_enum_variant(void *raw, int64_t bits, int64_t type) {
  rinzu_managed_data_t *data = rinzu_context_data(raw);
  uint32_t result = 0;
  if (rinzu_managed_enum_variant(data, (uint64_t)bits, rinzu_data_type(type), &result))
    rinzu_failure_fatal();
  return (int64_t)result;
}
int64_t rinzu_baseline_value_compare(void *raw, int64_t type, int64_t operation, int64_t left,
                                     int64_t right) {
  rinzu_managed_data_t *data = rinzu_context_data(raw);
  size_t index = rinzu_data_type(type);
  if (operation != 0 && operation != 1)
    rinzu_failure_fatal();
  int equal = 0;
  if (rinzu_managed_values_equal(data, index, rinzu_data_slot(index, (uint64_t)left),
                                 rinzu_data_slot(index, (uint64_t)right), &equal))
    rinzu_failure_fatal();
  return equal ^ (operation == 1);
}
int64_t rinzu_baseline_bytes(void *raw, const unsigned char *bytes, int64_t length) {
  RinzuContext *context = raw;
  if (length < 0)
    rinzu_failure_fatal();
  uint64_t result = rinzu_managed_bytes_literal(rinzu_context_data(context), bytes, (size_t)length);
  context->allocations += 1;
  return (int64_t)result;
}
int64_t rinzu_baseline_bytes_pad(void *raw, int64_t bits, int64_t from_bits, int64_t to_bits) {
  RinzuContext *context = raw;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  uint64_t from = (uint64_t)from_bits, to = (uint64_t)to_bits;
  rinzu_managed_value_t *value = rinzu_managed_find(data, (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_BYTES || value->length != from || to < from || to >= SIZE_MAX)
    rinzu_failure_fatal();
  rinzu_managed_value_t *padded = rinzu_managed_bytes_allocate_owned(data, (size_t)to);
  if (from)
    memcpy(padded->bytes, value->bytes, (size_t)from);
  context->allocations += 1;
  return (int64_t)padded->identity;
}
int64_t rinzu_baseline_bytes_length(void *raw, int64_t bits, int64_t expected) {
  rinzu_managed_value_t *value = rinzu_managed_find(rinzu_context_data(raw), (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_BYTES ||
      ((uint64_t)expected != UINT64_MAX && value->length != (uint64_t)expected))
    rinzu_failure_fatal();
  return (int64_t)value->length;
}

/* Private C compiler contract: selected Data/Owner was checked earlier in the
   same closed operation region. Keep exact Domain membership and typed shape. */
static inline rinzu_managed_value_t *rinzu_aot_find_owner_proven(void *raw, uint64_t bits) {
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(rinzu_context_data(raw), bits);
  if (!value)
    rinzu_failure_fatal();
  return value;
}
int64_t rinzu_baseline_bytes_int_length_owner_proven(void *raw, int64_t bits) {
  rinzu_managed_value_t *value = rinzu_aot_find_owner_proven(raw, (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_BYTES || value->length > INT64_MAX)
    rinzu_failure_fatal();
  return (int64_t)value->length;
}
int64_t rinzu_baseline_string_scalar_length_owner_proven(void *raw, int64_t bits) {
  rinzu_managed_value_t *value = rinzu_aot_find_owner_proven(raw, (uint64_t)bits);
  if (value->kind != RINZU_MANAGED_STRING || value->scalar_length > INT64_MAX)
    rinzu_failure_fatal();
  return (int64_t)value->scalar_length;
}
int64_t rinzu_baseline_project_owner_proven(void *raw, int64_t bits, int64_t type, int64_t variant,
                                            int64_t field) {
  rinzu_managed_data_t *data = rinzu_context_data(raw);
  size_t index = rinzu_data_type(type);
  if (field < 0)
    rinzu_failure_fatal();
  rinzu_managed_slot_t result = {0};
  int status = rinzu_managed_project_owned(data, (uint64_t)bits, index, variant, (size_t)field, &result);
  if (status == RINZU_VALUE_BUSY)
    return rinzu_aot_record_busy(raw);
  if (status)
    rinzu_failure_fatal();
  return (int64_t)result.bits;
}
