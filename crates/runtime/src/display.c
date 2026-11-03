#if !defined(__APPLE__) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif
#include "rinzu/display.h"
#include "rinzu/scalar.h"
#include <inttypes.h>
#include <locale.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

typedef struct {
  unsigned char *bytes;
  size_t length, capacity;
} rinzu_display_buffer_t;
static void *rinzu_display_grow(void *old, size_t count, size_t size) {
  if (count > SIZE_MAX / size)
    _exit(71);
  void *next = realloc(old, count * size);
  if (!next)
    _exit(71);
  return next;
}
static size_t rinzu_display_capacity(size_t old, size_t needed) {
  size_t next = old ? old : 32;
  while (next < needed) {
    if (next > SIZE_MAX / 2)
      return needed;
    next *= 2;
  }
  return next;
}
static void rinzu_display_append(rinzu_display_buffer_t *buffer, const void *bytes, size_t length) {
  if (length > SIZE_MAX - buffer->length)
    _exit(71);
  size_t needed = buffer->length + length;
  if (needed > buffer->capacity) {
    buffer->capacity = rinzu_display_capacity(buffer->capacity, needed);
    buffer->bytes = rinzu_display_grow(buffer->bytes, buffer->capacity, 1);
  }
  if (length)
    memcpy(buffer->bytes + buffer->length, bytes, length);
  buffer->length = needed;
}
static void rinzu_display_literal(rinzu_display_buffer_t *buffer, const char *text) {
  rinzu_display_append(buffer, text, strlen(text));
}
static void rinzu_display_quote(rinzu_display_buffer_t *buffer, rinzu_display_text_t text,
                                int bytes) {
  rinzu_display_literal(buffer, bytes ? "b\"" : "\"");
  for (size_t index = 0; index < text.length; ++index) {
    unsigned char byte = text.bytes[index];
    if (byte == '"' || byte == '\\') {
      unsigned char escaped[2] = {'\\', byte};
      rinzu_display_append(buffer, escaped, 2);
    } else if (!bytes && (byte == '\n' || byte == '\r' || byte == '\t')) {
      rinzu_display_literal(buffer, byte == '\n' ? "\\n" : byte == '\r' ? "\\r" : "\\t");
    } else if (byte < 32 || byte == 127 || (bytes && byte >= 128)) {
      static const char hex[] = "0123456789abcdef";
      char escaped[6] = {'\\', bytes ? 'x' : 'u', '{', hex[byte >> 4], hex[byte & 15], '}'};
      if (bytes) {
        escaped[2] = hex[byte >> 4];
        escaped[3] = hex[byte & 15];
      }
      rinzu_display_append(buffer, escaped, bytes ? 4 : 6);
    } else
      rinzu_display_append(buffer, &byte, 1);
  }
  rinzu_display_literal(buffer, "\"");
}
static pthread_once_t rinzu_display_locale_once = PTHREAD_ONCE_INIT;
static locale_t rinzu_display_locale;
static void rinzu_display_locale_init(void) {
  rinzu_display_locale = newlocale(LC_NUMERIC_MASK, "C", (locale_t)0);
  if (!rinzu_display_locale)
    _exit(71);
}
static int rinzu_display_scalar(rinzu_display_buffer_t *buffer, unsigned kind, uint64_t bits) {
  char text[128];
  int length;
  if (kind < 8) {
    if (rinzu_scalar_arithmetic(kind, 1, bits, 0).status)
      return 1;
    length = kind < 4 ? snprintf(text, sizeof text, "%" PRId64, (int64_t)bits)
                      : snprintf(text, sizeof text, "%" PRIu64, bits);
  } else if (kind == RINZU_SCALAR_BOOL) {
    if (bits > 1)
      return 1;
    rinzu_display_literal(buffer, bits ? "true" : "false");
    return 0;
  } else if (kind == RINZU_SCALAR_F32 || kind == RINZU_SCALAR_F64) {
    if (kind == RINZU_SCALAR_F32 && bits > UINT32_MAX)
      return 1;
    double value =
        kind == RINZU_SCALAR_F32 ? (double)rinzu_scalar_f32(bits) : rinzu_scalar_f64(bits);
    if (isnan(value)) {
      rinzu_display_literal(buffer, "NaN");
      return 0;
    }
    if (isinf(value)) {
      rinzu_display_literal(buffer, signbit(value) ? "-inf" : "inf");
      return 0;
    }
    pthread_once(&rinzu_display_locale_once, rinzu_display_locale_init);
    locale_t previous = uselocale(rinzu_display_locale);
    if (!previous)
      return 1;
    length = snprintf(text, sizeof text, kind == RINZU_SCALAR_F32 ? "%.9g" : "%.17g", value);
    if (!uselocale(previous))
      return 1;
    if (length > 0 && (size_t)length < sizeof text && !strpbrk(text, ".eE")) {
      text[length++] = '.';
      text[length++] = '0';
    }
  } else
    return 1;
  if (length < 0 || (size_t)length >= sizeof text)
    return 1;
  rinzu_display_append(buffer, text, (size_t)length);
  return 0;
}
typedef struct {
  unsigned text, owned;
  rinzu_display_text_t bytes;
  size_t type;
  rinzu_display_slot_t slot;
  int nested;
} rinzu_display_task_t;
typedef struct {
  rinzu_display_task_t *tasks;
  size_t count, capacity;
} rinzu_display_stack_t;
static void rinzu_display_push(rinzu_display_stack_t *stack, rinzu_display_task_t task) {
  if (stack->count == SIZE_MAX)
    _exit(71);
  if (stack->count == stack->capacity) {
    stack->capacity = rinzu_display_capacity(stack->capacity, stack->count + 1);
    stack->tasks = rinzu_display_grow(stack->tasks, stack->capacity, sizeof *stack->tasks);
  }
  stack->tasks[stack->count++] = task;
}
static void rinzu_display_push_text(rinzu_display_stack_t *stack, const char *text) {
  rinzu_display_push(stack, (rinzu_display_task_t){
                                .text = 1, .bytes = {(const unsigned char *)text, strlen(text)}});
}
typedef struct {
  unsigned char *key;
  size_t length;
  rinzu_display_slot_t value;
} rinzu_display_pair_t;
static int rinzu_display_pair_compare(const void *left, const void *right) {
  const rinzu_display_pair_t *a = left, *b = right;
  size_t count = a->length < b->length ? a->length : b->length;
  int order = count ? memcmp(a->key, b->key, count) : 0;
  return order ? order : a->length < b->length ? -1 : a->length > b->length;
}
int rinzu_display_format(const rinzu_display_type_t *types, size_t count, size_t type,
                         rinzu_display_slot_t slot, int nested, rinzu_display_load_t load,
                         void *context, unsigned char **bytes, size_t *length) {
  rinzu_display_buffer_t output = {0};
  rinzu_display_stack_t stack = {0};
  int status = 0;
  *bytes = NULL;
  *length = 0;
  rinzu_display_push(&stack, (rinzu_display_task_t){.type = type, .slot = slot, .nested = nested});
  while (stack.count && !status) {
    rinzu_display_task_t task = stack.tasks[--stack.count];
    if (task.text) {
      rinzu_display_append(&output, task.bytes.bytes, task.bytes.length);
      if (task.owned)
        free((void *)task.bytes.bytes);
      continue;
    }
    if (task.type >= count) {
      status = 1;
      break;
    }
    const rinzu_display_type_t *layout = &types[task.type];
    if (layout->kind == RINZU_DISPLAY_MARKER) {
      rinzu_display_append(&output, layout->name.bytes, layout->name.length);
      continue;
    }
    if (layout->kind == RINZU_DISPLAY_SCALAR) {
      status =
          task.slot.reference ? 1 : rinzu_display_scalar(&output, layout->scalar, task.slot.bits);
      continue;
    }
    if (layout->kind == RINZU_DISPLAY_UNIT) {
      if (task.slot.reference || task.slot.bits) {
        status = 1;
        break;
      }
      rinzu_display_literal(&output, "()");
      continue;
    }
    rinzu_display_node_t node = {0};
    if (!task.slot.reference || !load || load(context, task.type, task.slot.bits, &node)) {
      status = 1;
      break;
    }
    if (layout->kind == RINZU_DISPLAY_STRING || layout->kind == RINZU_DISPLAY_BYTES) {
      if (layout->kind == RINZU_DISPLAY_STRING && !task.nested)
        rinzu_display_append(&output, node.text.bytes, node.text.length);
      else
        rinzu_display_quote(&output, node.text, layout->kind == RINZU_DISPLAY_BYTES);
      continue;
    }
    if (layout->kind == RINZU_DISPLAY_ERROR) {
      rinzu_display_literal(&output, "Error(message: ");
      rinzu_display_quote(&output, node.text, 0);
      rinzu_display_literal(&output, ")");
      continue;
    }
    if (layout->kind == RINZU_DISPLAY_MAP) {
      if (node.count % 2) {
        status = 1;
        break;
      }
      size_t pairs = node.count / 2;
      rinzu_display_pair_t *sorted = pairs ? rinzu_display_grow(NULL, pairs, sizeof *sorted) : NULL;
      /* The load scratch can change while rendering a key. Copy all slots first.
       * Key types are closed integers/String/enums, never collections: this
       * helper call adds one bounded frame, independent of aggregate depth. */
      rinzu_display_slot_t *slots =
          node.count ? rinzu_display_grow(NULL, node.count, sizeof *slots) : NULL;
      if (node.count)
        memcpy(slots, node.fields, node.count * sizeof *slots);
      size_t prepared = 0;
      for (; prepared < pairs; ++prepared) {
        sorted[prepared].value = slots[prepared * 2 + 1];
        status = rinzu_display_format(types, count, layout->key, slots[prepared * 2], 1, load,
                                      context, &sorted[prepared].key, &sorted[prepared].length);
        if (status)
          break;
      }
      free(slots);
      if (status) {
        for (size_t i = 0; i < prepared; ++i)
          free(sorted[i].key);
        free(sorted);
        break;
      }
      if (pairs)
        qsort(sorted, pairs, sizeof *sorted, rinzu_display_pair_compare);
      rinzu_display_literal(&output, "{");
      rinzu_display_push_text(&stack, "}");
      for (size_t i = pairs; i > 0; --i) {
        if (i < pairs)
          rinzu_display_push_text(&stack, ", ");
        rinzu_display_push(&stack, (rinzu_display_task_t){.type = layout->value,
                                                          .slot = sorted[i - 1].value,
                                                          .nested = 1});
        rinzu_display_push_text(&stack, ": ");
        rinzu_display_push(
            &stack, (rinzu_display_task_t){
                        .text = 1, .owned = 1, .bytes = {sorted[i - 1].key, sorted[i - 1].length}});
      }
      free(sorted);
      continue;
    }
    const rinzu_display_field_t *fields = layout->fields;
    size_t expected = layout->count;
    const int anonymous_record = layout->kind == RINZU_DISPLAY_RECORD &&
                                 layout->name.length >= 14 &&
                                 memcmp(layout->name.bytes, "$rinzu_record_", 14) == 0;
    if (layout->kind == RINZU_DISPLAY_LIST) {
      rinzu_display_literal(&output, "[");
      rinzu_display_push_text(&stack, "]");
      expected = node.count;
    } else if (layout->kind == RINZU_DISPLAY_RECORD || layout->kind == RINZU_DISPLAY_ENUM) {
      if (!anonymous_record)
        rinzu_display_append(&output, layout->name.bytes, layout->name.length);
      if (layout->kind == RINZU_DISPLAY_ENUM) {
        if (node.variant >= layout->count) {
          status = 1;
          break;
        }
        const rinzu_display_variant_t *variant = &layout->variants[node.variant];
        rinzu_display_literal(&output, ".");
        rinzu_display_append(&output, variant->name.bytes, variant->name.length);
        fields = variant->fields;
        expected = variant->count;
        if (!expected && !node.count)
          continue;
      }
      rinzu_display_literal(&output, anonymous_record ? "{" : "(");
      rinzu_display_push_text(&stack, anonymous_record ? "}" : ")");
    } else {
      status = 1;
      break;
    }
    if (expected != node.count) {
      status = 1;
      break;
    }
    for (size_t i = node.count; i > 0; --i) {
      if (i < node.count)
        rinzu_display_push_text(&stack, ", ");
      size_t child = layout->kind == RINZU_DISPLAY_LIST ? layout->element : fields[i - 1].type;
      rinzu_display_push(
          &stack, (rinzu_display_task_t){.type = child, .slot = node.fields[i - 1], .nested = 1});
      if (layout->kind != RINZU_DISPLAY_LIST) {
        rinzu_display_push_text(&stack, anonymous_record ? "=" : ": ");
        rinzu_display_push(&stack, (rinzu_display_task_t){.text = 1, .bytes = fields[i - 1].name});
      }
    }
  }
  for (size_t i = 0; i < stack.count; ++i)
    if (stack.tasks[i].owned)
      free((void *)stack.tasks[i].bytes.bytes);
  free(stack.tasks);
  if (status)
    free(output.bytes);
  else {
    *bytes = output.bytes;
    *length = output.length;
  }
  return status;
}
