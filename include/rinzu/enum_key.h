#ifndef RINZU_ENUM_KEY_H
#define RINZU_ENUM_KEY_H
#include "rinzu/key.h"
#include "rinzu/failure.h"

enum {
  RINZU_KEY_NOT_KEY,
  RINZU_KEY_INTEGER,
  RINZU_KEY_STRING,
  RINZU_KEY_ENUM,
  RINZU_KEY_BOOL,
  RINZU_KEY_BYTES,
  RINZU_KEY_RECORD
};
enum { RINZU_KEY_OK, RINZU_KEY_EXCLUDED, RINZU_KEY_VALUE, RINZU_KEY_SCHEMA };
typedef struct {
  size_t count;
  const size_t *types;
} rinzu_key_variant_t;
typedef struct {
  unsigned kind, scalar_kind;
  const char *identity;
  size_t variant_count;
  const rinzu_key_variant_t *variants;
  unsigned bytes_fixed;
  uint64_t bytes_length;
  size_t field_count;
  const size_t *fields;
} rinzu_key_type_t;
typedef struct {
  const rinzu_key_type_t *types;
  size_t count;
  unsigned char *allowed;
} rinzu_key_schema_t;
/* Native views exist only while their Domain's values are rooted. Schema
   tables are immutable compiler metadata and contain no managed references. */
typedef struct rinzu_key_value {
  unsigned kind, scalar_kind;
  uint64_t bits;
  const unsigned char *bytes;
  size_t length;
  const char *identity;
  uint32_t variant;
  size_t count;
  const struct rinzu_key_value *fields;
} rinzu_key_value_t;
/* A scalar work item encodes enter/finish, avoiding a reused field cursor. */
static inline void rinzu_key_type_push(size_t **stack, size_t *depth, size_t *capacity,
                                       size_t item) {
  if (*depth == *capacity) {
    if (*capacity > SIZE_MAX / 2 / sizeof(**stack))
      rinzu_failure_fatal();
    size_t next = *capacity ? *capacity * 2 : 16;
    size_t *grown = rinzu_failure_allocate(next * sizeof(*grown));
    if (*depth)
      memcpy(grown, *stack, *depth * sizeof(*grown));
    free(*stack);
    *stack = grown;
    *capacity = next;
  }
  (*stack)[(*depth)++] = item;
}
static inline int rinzu_key_dependency(const rinzu_key_type_t *type, size_t *variant, size_t *field,
                                       size_t *child) {
  if (type->kind == RINZU_KEY_RECORD && *field < type->field_count) {
    *child = type->fields[(*field)++];
    return 1;
  }
  if (type->kind == RINZU_KEY_ENUM) {
    while (*variant < type->variant_count) {
      const rinzu_key_variant_t *fields = &type->variants[*variant];
      if (*field < fields->count) {
        *child = fields->types[(*field)++];
        return 1;
      }
      *variant += 1;
      *field = 0;
    }
  }
  return 0;
}
static inline int rinzu_key_schema_init(rinzu_key_schema_t *destination,
                                        const rinzu_key_type_t *types, size_t count) {
  if (!destination || destination->types || destination->count || destination->allowed ||
      (count && !types) || count > SIZE_MAX / 2 || count > SIZE_MAX / sizeof(size_t))
    return RINZU_KEY_SCHEMA;
  for (size_t index = 0; index < count; ++index) {
    const rinzu_key_type_t *type = &types[index];
    if (type->kind > RINZU_KEY_RECORD ||
        (type->kind == RINZU_KEY_INTEGER && type->scalar_kind > RINZU_SCALAR_BOOL) ||
        (type->kind == RINZU_KEY_BYTES && type->bytes_fixed > 1))
      return RINZU_KEY_SCHEMA;
    if (type->kind != RINZU_KEY_ENUM && type->kind != RINZU_KEY_RECORD)
      continue;
    if (!type->identity || !*type->identity)
      return RINZU_KEY_SCHEMA;
    for (size_t other = 0; other < index; ++other)
      if ((types[other].kind == RINZU_KEY_ENUM || types[other].kind == RINZU_KEY_RECORD) &&
          !strcmp(type->identity, types[other].identity))
        return RINZU_KEY_SCHEMA;
    if (type->kind == RINZU_KEY_RECORD) {
      if (type->field_count && !type->fields)
        return RINZU_KEY_SCHEMA;
      for (size_t field = 0; field < type->field_count; ++field)
        if (type->fields[field] >= count)
          return RINZU_KEY_SCHEMA;
      continue;
    }
    if (type->variant_count && !type->variants)
      return RINZU_KEY_SCHEMA;
    for (size_t variant = 0; variant < type->variant_count; ++variant) {
      const rinzu_key_variant_t *fields = &type->variants[variant];
      if (fields->count && !fields->types)
        return RINZU_KEY_SCHEMA;
      for (size_t field = 0; field < fields->count; ++field)
        if (fields->types[field] >= count)
          return RINZU_KEY_SCHEMA;
    }
  }
  rinzu_key_schema_t prepared = {types, count, count ? rinzu_failure_allocate(count) : NULL};
  unsigned char *colors = count ? rinzu_failure_allocate(count) : NULL;
  size_t capacity = count;
  size_t *stack = count ? rinzu_failure_allocate(count * sizeof(*stack)) : NULL;
  for (size_t index = 0; index < count; ++index)
    prepared.allowed[index] =
        types[index].kind == RINZU_KEY_STRING || types[index].kind == RINZU_KEY_ENUM ||
        types[index].kind == RINZU_KEY_BOOL || types[index].kind == RINZU_KEY_BYTES ||
        types[index].kind == RINZU_KEY_RECORD ||
        (types[index].kind == RINZU_KEY_INTEGER && types[index].scalar_kind <= RINZU_SCALAR_U64);
  /* All variants participate, including variants absent from the current
     value. A dependency cycle is excluded, matching the compiler Key rule. */
  for (size_t root = 0; root < count; ++root) {
    if (colors[root])
      continue;
    size_t depth = 0;
    rinzu_key_type_push(&stack, &depth, &capacity, root * 2);
    while (depth) {
      size_t item = stack[--depth], node = item / 2;
      const rinzu_key_type_t *type = &types[node];
      size_t variant = 0, field = 0, child;
      if (item % 2) {
        while (rinzu_key_dependency(type, &variant, &field, &child))
          prepared.allowed[node] &= colors[child] == 2 && prepared.allowed[child];
        colors[node] = 2;
      } else if (!colors[node]) {
        colors[node] = 1;
        rinzu_key_type_push(&stack, &depth, &capacity, node * 2 + 1);
        while (rinzu_key_dependency(type, &variant, &field, &child)) {
          if (!colors[child])
            rinzu_key_type_push(&stack, &depth, &capacity, child * 2);
          else if (colors[child] == 1)
            prepared.allowed[node] = 0;
        }
      }
    }
  }
  free(stack);
  free(colors);
  *destination = prepared;
  return RINZU_KEY_OK;
}
static inline void rinzu_key_schema_dispose(rinzu_key_schema_t *schema) {
  free(schema->allowed);
  *schema = (rinzu_key_schema_t){0};
}
typedef struct {
  size_t type, field;
  const rinzu_key_value_t *value;
} rinzu_key_value_frame_t;
static inline int rinzu_key_validate(const rinzu_key_schema_t *schema, size_t root,
                                     const rinzu_key_value_t *value) {
  if (!schema || root >= schema->count || !schema->allowed || !value ||
      schema->count > SIZE_MAX / sizeof(rinzu_key_value_frame_t))
    return RINZU_KEY_SCHEMA;
  if (!schema->allowed[root])
    return RINZU_KEY_EXCLUDED;
  rinzu_key_value_frame_t *stack = rinzu_failure_allocate(schema->count * sizeof(*stack));
  size_t depth = 1;
  stack[0] = (rinzu_key_value_frame_t){root, 0, value};
  int status = RINZU_KEY_OK;
  while (depth && !status) {
    rinzu_key_value_frame_t *frame = &stack[depth - 1];
    const rinzu_key_type_t *type = &schema->types[frame->type];
    value = frame->value;
    if (value->kind != type->kind) {
      status = RINZU_KEY_VALUE;
      break;
    }
    if (type->kind == RINZU_KEY_INTEGER) {
      if (value->scalar_kind != type->scalar_kind ||
          !rinzu_transfer_valid(value->scalar_kind, value->bits))
        status = RINZU_KEY_VALUE;
      depth -= 1;
    } else if (type->kind == RINZU_KEY_BOOL) {
      if (value->scalar_kind != RINZU_SCALAR_BOOL || value->bits > 1)
        status = RINZU_KEY_VALUE;
      depth -= 1;
    } else if (type->kind == RINZU_KEY_BYTES) {
      if ((value->length && !value->bytes) ||
          (type->bytes_fixed && value->length != type->bytes_length))
        status = RINZU_KEY_VALUE;
      depth -= 1;
    } else if (type->kind == RINZU_KEY_STRING) {
      if (!rinzu_utf8_valid(value->bytes, value->length))
        status = RINZU_KEY_VALUE;
      depth -= 1;
    } else {
      if (!value->identity || strcmp(value->identity, type->identity) != 0 ||
          (value->count && !value->fields)) {
        status = RINZU_KEY_VALUE;
        break;
      }
      const size_t *fields;
      if (type->kind == RINZU_KEY_RECORD) {
        if (value->count != type->field_count) {
          status = RINZU_KEY_VALUE;
          break;
        }
        fields = type->fields;
      } else {
        if (value->variant >= type->variant_count ||
            value->count != type->variants[value->variant].count) {
          status = RINZU_KEY_VALUE;
          break;
        }
        fields = type->variants[value->variant].types;
      }
      if (frame->field == value->count) {
        depth -= 1;
        continue;
      }
      size_t field = frame->field++;
      if (depth == schema->count) {
        status = RINZU_KEY_SCHEMA;
        break;
      }
      stack[depth++] = (rinzu_key_value_frame_t){fields[field], 0, &value->fields[field]};
    }
  }
  free(stack);
  return status;
}
static inline void rinzu_key_enum_header(rinzu_key_hasher_t *hash, const rinzu_key_value_t *value) {
  const unsigned char prefix = 0x30;
  size_t length = strlen(value->identity);
  rinzu_key_write(hash, &prefix, 1);
  rinzu_key_u64(hash, length, 8);
  rinzu_key_write(hash, (const unsigned char *)value->identity, length);
  rinzu_key_u64(hash, value->variant, 4);
  rinzu_key_u64(hash, value->count, 8);
}
static inline void rinzu_key_record_header(rinzu_key_hasher_t *hash,
                                           const rinzu_key_value_t *value) {
  const unsigned char prefix = 0x31;
  size_t length = strlen(value->identity);
  rinzu_key_write(hash, &prefix, 1);
  rinzu_key_u64(hash, length, 8);
  rinzu_key_write(hash, (const unsigned char *)value->identity, length);
  rinzu_key_u64(hash, value->count, 8);
}
static inline int rinzu_key_hash(const rinzu_key_schema_t *schema, size_t root,
                                 const rinzu_key_value_t *value, uint64_t *destination) {
  if (!destination)
    return RINZU_KEY_VALUE;
  int status = rinzu_key_validate(schema, root, value);
  if (status)
    return status;
  /* Hash whole pending values, independent of sibling depth or field count. */
  size_t capacity = schema->count, depth = 1;
  const rinzu_key_value_t **stack = rinzu_failure_allocate(capacity * sizeof(*stack));
  stack[0] = value;
  rinzu_key_hasher_t hash = rinzu_key_init();
  while (depth) {
    value = stack[--depth];
    if (value->kind == RINZU_KEY_INTEGER)
      (void)rinzu_key_integer(&hash, value->scalar_kind, value->bits);
    else if (value->kind == RINZU_KEY_BOOL)
      (void)rinzu_key_bool(&hash, value->bits);
    else if (value->kind == RINZU_KEY_BYTES)
      (void)rinzu_key_bytes(&hash, value->bytes, value->length);
    else if (value->kind == RINZU_KEY_STRING)
      (void)rinzu_key_string(&hash, value->bytes, value->length);
    else {
      if (value->kind == RINZU_KEY_RECORD)
        rinzu_key_record_header(&hash, value);
      else
        rinzu_key_enum_header(&hash, value);
      if (value->count > SIZE_MAX - depth || depth + value->count > SIZE_MAX / sizeof(*stack))
        rinzu_failure_fatal();
      size_t required = depth + value->count;
      if (required > capacity) {
        size_t next = capacity <= SIZE_MAX / 2 / sizeof(*stack) ? capacity * 2 : required;
        if (next < required)
          next = required;
        const rinzu_key_value_t **grown = rinzu_failure_allocate(next * sizeof(*grown));
        if (depth)
          memcpy(grown, stack, depth * sizeof(*grown));
        free(stack);
        stack = grown;
        capacity = next;
      }
      for (size_t field = value->count; field; --field)
        stack[depth++] = &value->fields[field - 1];
    }
  }
  free(stack);
  *destination = hash.hash;
  return RINZU_KEY_OK;
}
typedef struct {
  size_t field;
  const rinzu_key_value_t *left, *right;
} rinzu_key_pair_frame_t;
static inline int rinzu_key_equal(const rinzu_key_schema_t *schema, size_t root,
                                  const rinzu_key_value_t *left, const rinzu_key_value_t *right,
                                  int *destination) {
  if (!destination)
    return RINZU_KEY_VALUE;
  int status = rinzu_key_validate(schema, root, left);
  if (status)
    return status;
  status = rinzu_key_validate(schema, root, right);
  if (status)
    return status;
  if (schema->count > SIZE_MAX / sizeof(rinzu_key_pair_frame_t))
    return RINZU_KEY_SCHEMA;
  rinzu_key_pair_frame_t *stack = rinzu_failure_allocate(schema->count * sizeof(*stack));
  size_t depth = 1;
  stack[0] = (rinzu_key_pair_frame_t){0, left, right};
  int equal = 1;
  while (depth && equal) {
    rinzu_key_pair_frame_t *frame = &stack[depth - 1];
    left = frame->left;
    right = frame->right;
    if (left->kind == RINZU_KEY_INTEGER || left->kind == RINZU_KEY_BOOL) {
      equal = left->bits == right->bits;
      depth -= 1;
    } else if (left->kind == RINZU_KEY_STRING || left->kind == RINZU_KEY_BYTES) {
      equal = left->length == right->length &&
              (!left->length || !memcmp(left->bytes, right->bytes, left->length));
      depth -= 1;
    } else {
      if (left->kind == RINZU_KEY_ENUM && left->variant != right->variant) {
        equal = 0;
        break;
      }
      if (frame->field == left->count) {
        depth -= 1;
        continue;
      }
      size_t field = frame->field++;
      stack[depth++] = (rinzu_key_pair_frame_t){0, &left->fields[field], &right->fields[field]};
    }
  }
  free(stack);
  *destination = equal;
  return RINZU_KEY_OK;
}
#endif
