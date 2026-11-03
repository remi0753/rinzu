#ifndef RINZU_VALUE_H
#define RINZU_VALUE_H
#include "rinzu/enum_key.h"
#include "rinzu/error_value.h"

enum {
  RINZU_VALUE_SCALAR,
  RINZU_VALUE_UNIT,
  RINZU_VALUE_STRING,
  RINZU_VALUE_BYTES,
  RINZU_VALUE_RECORD,
  RINZU_VALUE_ENUM,
  RINZU_VALUE_LIST,
  RINZU_VALUE_MAP,
  RINZU_VALUE_OTHER,
  RINZU_VALUE_DYNAMIC_BYTES,
  RINZU_VALUE_ERROR,
  RINZU_VALUE_STATE,
  RINZU_VALUE_ERROR_RECORD
};
/* Private closed-schema tag; Other remains unequal and nontransferable. */
#define RINZU_VALUE_OTHER_FUTURE 1
enum {
  RINZU_VALUE_OK,
  RINZU_VALUE_SCHEMA,
  RINZU_VALUE_TYPE,
  RINZU_VALUE_NO_EQUALITY,
  RINZU_VALUE_NOT_KEY,
  RINZU_VALUE_DUPLICATE,
  RINZU_VALUE_BUSY
};
typedef struct {
  unsigned kind, scalar_kind;
  const char *identity;
  uint64_t bytes_length;
  size_t field_count;
  const size_t *fields;
  size_t variant_count;
  const rinzu_key_variant_t *variants;
  size_t element, key, value;
} rinzu_value_type_t;
typedef struct {
  const rinzu_value_type_t *types;
  size_t count;
  unsigned char *equality, *transferable;
  rinzu_key_type_t *key_types;
  rinzu_key_schema_t keys;
} rinzu_value_schema_t;
typedef struct rinzu_value_pair rinzu_value_pair_t;
typedef struct rinzu_value {
  unsigned kind, scalar_kind;
  uint64_t bits;
  const unsigned char *bytes;
  size_t length;
  const char *identity;
  uint32_t variant;
  size_t count;
  const struct rinzu_value *fields;
  const rinzu_value_pair_t *entries;
  size_t other_type;
} rinzu_value_t;
struct rinzu_value_pair {
  rinzu_value_t key, value;
};
typedef struct {
  rinzu_value_pair_t *entries;
  size_t count;
} rinzu_value_map_t;
typedef struct {
  size_t type, group, field;
} rinzu_value_type_frame_t;

static inline int rinzu_value_dependency(const rinzu_value_type_t *type, size_t *group,
                                         size_t *field, size_t *child) {
  if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD) {
    if (*field == type->field_count)
      return 0;
    *child = type->fields[(*field)++];
    return 1;
  }
  if (type->kind == RINZU_VALUE_ENUM) {
    while (*group < type->variant_count) {
      const rinzu_key_variant_t *variant = &type->variants[*group];
      if (*field < variant->count) {
        *child = variant->types[(*field)++];
        return 1;
      }
      *group += 1;
      *field = 0;
    }
    return 0;
  }
  if (type->kind == RINZU_VALUE_LIST && *field == 0) {
    *field = 1;
    *child = type->element;
    return 1;
  }
  if (type->kind == RINZU_VALUE_MAP && *field < 2) {
    *child = (*field)++ ? type->value : type->key;
    return 1;
  }
  return 0;
}
static inline int rinzu_value_schema_init(rinzu_value_schema_t *destination,
                                          const rinzu_value_type_t *types, size_t count) {
  if (!destination || destination->types || destination->count || destination->equality ||
      destination->key_types || destination->keys.allowed || (count && !types) ||
      count > SIZE_MAX / sizeof(rinzu_value_type_frame_t) ||
      count > SIZE_MAX / sizeof(rinzu_key_type_t))
    return RINZU_VALUE_SCHEMA;
  for (size_t index = 0; index < count; ++index) {
    const rinzu_value_type_t *type = &types[index];
    if (type->kind > RINZU_VALUE_ERROR_RECORD ||
        (type->kind == RINZU_VALUE_SCALAR && type->scalar_kind > RINZU_SCALAR_BOOL))
      return RINZU_VALUE_SCHEMA;
    if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD ||
        type->kind == RINZU_VALUE_ENUM) {
      if (!type->identity || !*type->identity)
        return RINZU_VALUE_SCHEMA;
      for (size_t other = 0; other < index; ++other)
        if ((types[other].kind == RINZU_VALUE_RECORD || types[other].kind == RINZU_VALUE_STATE || types[other].kind == RINZU_VALUE_ERROR_RECORD ||
             types[other].kind == RINZU_VALUE_ENUM) &&
            !strcmp(type->identity, types[other].identity))
          return RINZU_VALUE_SCHEMA;
    }
    if ((type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD) &&
        type->field_count && !type->fields)
      return RINZU_VALUE_SCHEMA;
    if (type->kind == RINZU_VALUE_ENUM) {
      if (type->variant_count && !type->variants)
        return RINZU_VALUE_SCHEMA;
      for (size_t variant = 0; variant < type->variant_count; ++variant)
        if (type->variants[variant].count && !type->variants[variant].types)
          return RINZU_VALUE_SCHEMA;
    }
    size_t group = 0, field = 0, child;
    while (rinzu_value_dependency(type, &group, &field, &child))
      if (child >= count)
        return RINZU_VALUE_SCHEMA;
  }
  for (size_t index = 0; index < count; ++index) {
    const rinzu_value_type_t *type = &types[index];
    if (type->kind == RINZU_VALUE_ERROR_RECORD &&
        (type->field_count < 4 || type->field_count > 5 ||
         types[type->fields[0]].kind != RINZU_VALUE_STRING ||
         types[type->fields[1]].kind != RINZU_VALUE_STRING))
      return RINZU_VALUE_SCHEMA;
  }
  rinzu_value_schema_t prepared = {0};
  prepared.types = types;
  prepared.count = count;
  prepared.key_types = count ? rinzu_failure_allocate(count * sizeof(*prepared.key_types)) : NULL;
  for (size_t index = 0; index < count; ++index) {
    const rinzu_value_type_t *type = &types[index];
    rinzu_key_type_t *key = &prepared.key_types[index];
    if (type->kind == RINZU_VALUE_SCALAR) {
      key->kind = type->scalar_kind == RINZU_SCALAR_BOOL ? RINZU_KEY_BOOL : RINZU_KEY_INTEGER;
      key->scalar_kind = type->scalar_kind;
    } else if (type->kind == RINZU_VALUE_STRING)
      key->kind = RINZU_KEY_STRING;
    else if (type->kind == RINZU_VALUE_BYTES || type->kind == RINZU_VALUE_DYNAMIC_BYTES) {
      key->kind = RINZU_KEY_BYTES;
      key->bytes_fixed = type->kind == RINZU_VALUE_BYTES;
      key->bytes_length = type->bytes_length;
    } else if (type->kind == RINZU_VALUE_RECORD) {
      key->kind = RINZU_KEY_RECORD;
      key->identity = type->identity;
      key->field_count = type->field_count;
      key->fields = type->fields;
    } else if (type->kind == RINZU_VALUE_ENUM) {
      key->kind = RINZU_KEY_ENUM;
      key->identity = type->identity;
      key->variant_count = type->variant_count;
      key->variants = type->variants;
    }
  }
  if (rinzu_key_schema_init(&prepared.keys, prepared.key_types, count)) {
    free(prepared.key_types);
    return RINZU_VALUE_SCHEMA;
  }
  for (size_t index = 0; index < count; ++index)
    if (types[index].kind == RINZU_VALUE_MAP && !prepared.keys.allowed[types[index].key]) {
      rinzu_key_schema_dispose(&prepared.keys);
      free(prepared.key_types);
      return RINZU_VALUE_SCHEMA;
    }
  prepared.equality = count ? rinzu_failure_allocate(count) : NULL;
  prepared.transferable = count ? rinzu_failure_allocate(count) : NULL;
  unsigned char *colors = count ? rinzu_failure_allocate(count) : NULL;
  rinzu_value_type_frame_t *stack = count ? rinzu_failure_allocate(count * sizeof(*stack)) : NULL;
  for (size_t index = 0; index < count; ++index)
    prepared.equality[index] = types[index].kind != RINZU_VALUE_OTHER &&
                               types[index].kind != RINZU_VALUE_STATE &&
                               types[index].kind != RINZU_VALUE_ERROR_RECORD &&
                               types[index].kind != RINZU_VALUE_ERROR;
  for (size_t index = 0; index < count; ++index)
    prepared.transferable[index] =
        types[index].kind != RINZU_VALUE_OTHER && types[index].kind != RINZU_VALUE_STATE && types[index].kind != RINZU_VALUE_ERROR_RECORD;
  for (size_t root = 0; root < count; ++root) {
    if (colors[root])
      continue;
    size_t depth = 1;
    colors[root] = 1;
    stack[0] = (rinzu_value_type_frame_t){root, 0, 0};
    while (depth) {
      rinzu_value_type_frame_t *frame = &stack[depth - 1];
      size_t group = frame->group, field = frame->field, child;
      if (rinzu_value_dependency(&types[frame->type], &group, &field, &child)) {
        if (!colors[child]) {
          if (depth == count)
            rinzu_failure_fatal();
          colors[child] = 1;
          stack[depth++] = (rinzu_value_type_frame_t){child, 0, 0};
        } else {
          prepared.equality[frame->type] &= colors[child] == 2 && prepared.equality[child];
          prepared.transferable[frame->type] &= colors[child] == 2 && prepared.transferable[child];
          frame->group = group;
          frame->field = field;
        }
      } else {
        colors[frame->type] = 2;
        depth -= 1;
      }
    }
  }
  free(colors);
  free(stack);
  *destination = prepared;
  return RINZU_VALUE_OK;
}
static inline void rinzu_value_schema_dispose(rinzu_value_schema_t *schema) {
  free(schema->equality);
  free(schema->transferable);
  rinzu_key_schema_dispose(&schema->keys);
  free(schema->key_types);
  *schema = (rinzu_value_schema_t){0};
}
static inline int rinzu_value_validate(const rinzu_value_schema_t *, size_t, const rinzu_value_t *);
static inline int rinzu_value_equal(const rinzu_value_schema_t *, size_t, const rinzu_value_t *,
                                    const rinzu_value_t *, int *);
static inline int rinzu_value_key_hash(const rinzu_value_schema_t *, size_t, const rinzu_value_t *,
                                       uint64_t *);

typedef struct {
  uint64_t hash;
  size_t entry_plus_one;
} rinzu_value_map_bucket_t;
typedef struct {
  size_t capacity;
  rinzu_value_map_bucket_t buckets[];
} rinzu_value_map_index_t;
static inline rinzu_value_map_index_t *rinzu_value_map_index_new(size_t count) {
  size_t capacity = 4;
  while (count > capacity / 2) {
    if (capacity > SIZE_MAX / 2)
      rinzu_failure_fatal();
    capacity *= 2;
  }
  if (capacity > (SIZE_MAX - sizeof(rinzu_value_map_index_t)) / sizeof(rinzu_value_map_bucket_t))
    rinzu_failure_fatal();
  rinzu_value_map_index_t *index =
      rinzu_failure_allocate(sizeof(*index) + capacity * sizeof(*index->buckets));
  index->capacity = capacity;
  return index;
}
/* A borrowed view index. Hash collisions always compare the complete typed
   key; entries remain owned by the caller for the entire validation/walk. */
static inline int rinzu_value_map_probe(const rinzu_value_schema_t *schema, size_t key_type,
    const rinzu_value_pair_t *entries, rinzu_value_map_index_t *index,
    const rinzu_value_t *key, uint64_t hash, size_t *bucket, size_t *matched) {
  *matched = SIZE_MAX;
  size_t cursor = (size_t)hash & (index->capacity - 1);
  while (index->buckets[cursor].entry_plus_one) {
    rinzu_value_map_bucket_t candidate = index->buckets[cursor];
    if (candidate.hash == hash) {
      int equal = 0;
      int status = rinzu_value_equal(schema,key_type,key,
          &entries[candidate.entry_plus_one - 1].key,&equal);
      if (status)
        return status;
      if (equal) {
        *matched = candidate.entry_plus_one - 1;
        break;
      }
    }
    cursor = (cursor + 1) & (index->capacity - 1);
  }
  *bucket = cursor;
  return RINZU_VALUE_OK;
}
typedef struct {
  size_t type, field;
  const rinzu_value_t *value;
} rinzu_value_frame_t;
typedef struct {
  size_t type, field;
  const rinzu_value_t *value;
  rinzu_value_map_index_t *map_index;
} rinzu_value_validation_frame_t;
/* -1 means that a composite walk is required, never a public status. */
static inline int rinzu_value_validate_leaf(const rinzu_value_type_t *leaf,
                                           const rinzu_value_t *value) {
  if (leaf->kind == RINZU_VALUE_SCALAR)
    return value->kind == leaf->kind && value->scalar_kind == leaf->scalar_kind &&
                   rinzu_transfer_valid(value->scalar_kind, value->bits)
               ? RINZU_VALUE_OK : RINZU_VALUE_TYPE;
  if (leaf->kind == RINZU_VALUE_UNIT)
    return value->kind == leaf->kind ? RINZU_VALUE_OK : RINZU_VALUE_TYPE;
  if (leaf->kind == RINZU_VALUE_STRING || leaf->kind == RINZU_VALUE_BYTES ||
      leaf->kind == RINZU_VALUE_DYNAMIC_BYTES)
    return value->kind == leaf->kind && (!value->length || value->bytes) &&
                   (leaf->kind != RINZU_VALUE_DYNAMIC_BYTES || value->length <= INT64_MAX) &&
                   (leaf->kind != RINZU_VALUE_BYTES || value->length == leaf->bytes_length) &&
                   (leaf->kind != RINZU_VALUE_STRING || rinzu_utf8_valid(value->bytes,value->length))
               ? RINZU_VALUE_OK : RINZU_VALUE_TYPE;
  if (value->kind == leaf->kind && !value->count &&
      (leaf->kind == RINZU_VALUE_MAP || leaf->kind == RINZU_VALUE_LIST))
    return RINZU_VALUE_OK;
  return -1;
}
static inline int rinzu_value_validate(const rinzu_value_schema_t *schema, size_t root,
                                       const rinzu_value_t *value) {
  if (!schema || root >= schema->count || !schema->types || !value ||
      schema->count > SIZE_MAX / sizeof(rinzu_value_validation_frame_t))
    return RINZU_VALUE_SCHEMA;
  int leaf_status = rinzu_value_validate_leaf(&schema->types[root],value);
  if (leaf_status >= 0)
    return leaf_status;
  /* Equality metadata can contain cycles with finite values (e.g. empty List).
     Grow an explicit walk stack without adding a source-visible depth limit. */
  size_t capacity = 1, depth = 1;
  rinzu_value_validation_frame_t first;
  rinzu_value_validation_frame_t *stack = &first;
  stack[0] = (rinzu_value_validation_frame_t){root, 0, value, NULL};
  int status = RINZU_VALUE_OK;
  while (depth && !status) {
    rinzu_value_validation_frame_t *frame = &stack[depth - 1];
    const rinzu_value_type_t *type = &schema->types[frame->type];
    value = frame->value;
    if (type->kind == RINZU_VALUE_ERROR && value->kind == RINZU_VALUE_ERROR_RECORD &&
        value->other_type < schema->count && schema->types[value->other_type].kind == RINZU_VALUE_ERROR_RECORD) {
      depth -= 1;
      continue;
    }
    if (value->kind != type->kind) {
      status = RINZU_VALUE_TYPE;
      break;
    }
    if (type->kind == RINZU_VALUE_SCALAR) {
      if (value->scalar_kind != type->scalar_kind ||
          !rinzu_transfer_valid(value->scalar_kind, value->bits))
        status = RINZU_VALUE_TYPE;
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_UNIT) {
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_ERROR) {
      if (!rinzu_error_value_valid(value->bytes, value->length))
        status = RINZU_VALUE_TYPE;
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_STRING || type->kind == RINZU_VALUE_BYTES ||
        type->kind == RINZU_VALUE_DYNAMIC_BYTES) {
      if ((value->length && !value->bytes) ||
          (type->kind == RINZU_VALUE_DYNAMIC_BYTES && value->length > INT64_MAX) ||
          (type->kind == RINZU_VALUE_BYTES && value->length != type->bytes_length) ||
          (type->kind == RINZU_VALUE_STRING && !rinzu_utf8_valid(value->bytes, value->length)))
        status = RINZU_VALUE_TYPE;
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_OTHER || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD) {
      if (value->other_type != frame->type)
        status = RINZU_VALUE_TYPE;
      depth -= 1;
      continue;
    }
    size_t child_type;
    const rinzu_value_t *child;
    if (type->kind == RINZU_VALUE_MAP) {
      if (value->count && !value->entries) {
        status = RINZU_VALUE_TYPE;
        break;
      }
      if (value->count > SIZE_MAX / 2) {
        status = RINZU_VALUE_TYPE;
        break;
      }
      if (frame->field == value->count * 2) {
        free(frame->map_index);
        frame->map_index = NULL;
        depth -= 1;
        continue;
      }
      size_t entry = frame->field / 2;
      int is_value = frame->field & 1;
      child_type = is_value ? type->value : type->key;
      child = is_value ? &value->entries[entry].value : &value->entries[entry].key;
      if (!is_value) {
        uint64_t hash;
        status = rinzu_value_key_hash(schema, type->key, child, &hash);
        if (status)
          break;
        if (value->count > 1) {
          if (!frame->map_index)
            frame->map_index = rinzu_value_map_index_new(value->count);
          size_t bucket, matched;
          status = rinzu_value_map_probe(schema,type->key,value->entries,frame->map_index,
                                         child,hash,&bucket,&matched);
          if (status)
            break;
          if (matched != SIZE_MAX) {
            status = RINZU_VALUE_DUPLICATE;
            break;
          }
          frame->map_index->buckets[bucket] = (rinzu_value_map_bucket_t){hash,entry + 1};
        }
      }
    } else {
      if (value->count && !value->fields) {
        status = RINZU_VALUE_TYPE;
        break;
      }
      const size_t *fields = NULL;
      size_t count = value->count;
      if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_ENUM) {
        if (!value->identity || strcmp(value->identity, type->identity) != 0) {
          status = RINZU_VALUE_TYPE;
          break;
        }
        if (type->kind == RINZU_VALUE_RECORD) {
          fields = type->fields;
          count = type->field_count;
        } else {
          if (value->variant >= type->variant_count) {
            status = RINZU_VALUE_TYPE;
            break;
          }
          fields = type->variants[value->variant].types;
          count = type->variants[value->variant].count;
        }
        if (value->count != count) {
          status = RINZU_VALUE_TYPE;
          break;
        }
      }
      if (frame->field == count) {
        depth -= 1;
        continue;
      }
      // Validated record/enum schemas have nonnull fields whenever count > 0.
      // Schema validation is outside this analyzer path's inlining budget.
      // NOLINTNEXTLINE(clang-analyzer-core.NullDereference)
      child_type = type->kind == RINZU_VALUE_LIST ? type->element : fields[frame->field];
      child = &value->fields[frame->field];
    }
    frame->field += 1;
    int child_status = rinzu_value_validate_leaf(&schema->types[child_type],child);
    if (child_status >= 0) {
      status = child_status;
      continue;
    }
    if (depth == capacity) {
      if (capacity > SIZE_MAX / 2 / sizeof(*stack))
        rinzu_failure_fatal();
      rinzu_value_validation_frame_t *grown = rinzu_failure_allocate(capacity * 2 * sizeof(*stack));
      memcpy(grown, stack, depth * sizeof(*stack));
      if (stack != &first)
        free(stack);
      stack = grown;
      capacity *= 2;
    }
    stack[depth++] = (rinzu_value_validation_frame_t){child_type, 0, child, NULL};
  }
  for (size_t index = 0; index < depth; ++index)
    free(stack[index].map_index);
  if (stack != &first)
    free(stack);
  return status;
}
/* Called only after complete typed validation. In particular String UTF-8
   need not be decoded a second time just to hash its bytes. */
static inline int rinzu_value_hash_leaf(rinzu_key_hasher_t *hash, const rinzu_value_t *value) {
  if (value->kind == RINZU_VALUE_SCALAR) {
    if (value->scalar_kind == RINZU_SCALAR_BOOL)
      (void)rinzu_key_bool(hash, value->bits);
    else
      (void)rinzu_key_integer(hash, value->scalar_kind, value->bits);
    return 1;
  }
  if (value->kind == RINZU_VALUE_STRING || value->kind == RINZU_VALUE_BYTES ||
      value->kind == RINZU_VALUE_DYNAMIC_BYTES) {
    const unsigned char prefix = value->kind == RINZU_VALUE_STRING ? 0x20 : 0x21;
    rinzu_key_write(hash, &prefix, 1);
    rinzu_key_u64(hash, value->length, 8);
    rinzu_key_write(hash, value->bytes, value->length);
    return 1;
  }
  return 0;
}
static inline int rinzu_value_key_hash(const rinzu_value_schema_t *schema, size_t root,
                                       const rinzu_value_t *value, uint64_t *destination) {
  if (!schema || root >= schema->count || !schema->keys.allowed || !destination)
    return RINZU_VALUE_SCHEMA;
  if (!schema->keys.allowed[root])
    return RINZU_VALUE_NOT_KEY;
  int status = rinzu_value_validate(schema, root, value);
  if (status)
    return status;
  rinzu_key_hasher_t hash = rinzu_key_init();
  if (rinzu_value_hash_leaf(&hash, value)) {
    *destination = hash.hash;
    return RINZU_VALUE_OK;
  }
  /* Pending values preserve declaration order without reusing a sibling's
     field cursor. A wide aggregate may need more slots than schema nodes. */
  size_t capacity = schema->count, depth = 1;
  const rinzu_value_t **stack = rinzu_failure_allocate(capacity * sizeof(*stack));
  stack[0] = value;
  while (depth) {
    value = stack[--depth];
    if (!rinzu_value_hash_leaf(&hash, value)) {
      rinzu_key_value_t key = {
          .identity = value->identity, .variant = value->variant, .count = value->count};
      if (value->kind == RINZU_VALUE_RECORD)
        rinzu_key_record_header(&hash, &key);
      else
        rinzu_key_enum_header(&hash, &key);
      if (value->count > SIZE_MAX - depth || depth + value->count > SIZE_MAX / sizeof(*stack))
        rinzu_failure_fatal();
      size_t required = depth + value->count;
      if (required > capacity) {
        size_t next = capacity <= SIZE_MAX / 2 / sizeof(*stack) ? capacity * 2 : required;
        if (next < required)
          next = required;
        const rinzu_value_t **grown = rinzu_failure_allocate(next * sizeof(*grown));
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
  return RINZU_VALUE_OK;
}

typedef struct {
  size_t type, field;
  const rinzu_value_t *left, *right;
  rinzu_value_map_index_t *map_index;
} rinzu_value_pair_frame_t;
static inline int rinzu_value_equal(const rinzu_value_schema_t *schema, size_t root,
                                    const rinzu_value_t *left, const rinzu_value_t *right,
                                    int *destination) {
  if (!schema || root >= schema->count || !schema->equality || !destination ||
      schema->count > SIZE_MAX / sizeof(rinzu_value_pair_frame_t))
    return RINZU_VALUE_SCHEMA;
  if (!schema->equality[root])
    return RINZU_VALUE_NO_EQUALITY;
  int status = rinzu_value_validate(schema, root, left);
  if (status)
    return status;
  status = rinzu_value_validate(schema, root, right);
  if (status)
    return status;
  unsigned kind = schema->types[root].kind;
  if (kind == RINZU_VALUE_SCALAR) {
    *destination = rinzu_scalar_compare(schema->types[root].scalar_kind, 0, left->bits, right->bits);
    return RINZU_VALUE_OK;
  }
  if (kind == RINZU_VALUE_UNIT ||
      ((kind == RINZU_VALUE_MAP || kind == RINZU_VALUE_LIST) && !left->count && !right->count)) {
    *destination = 1;
    return RINZU_VALUE_OK;
  }
  if (kind == RINZU_VALUE_STRING || kind == RINZU_VALUE_BYTES || kind == RINZU_VALUE_DYNAMIC_BYTES) {
    *destination = left->length == right->length &&
                   (!left->length || !memcmp(left->bytes, right->bytes, left->length));
    return RINZU_VALUE_OK;
  }
  rinzu_value_pair_frame_t *stack = rinzu_failure_allocate(schema->count * sizeof(*stack));
  size_t depth = 1;
  stack[0] = (rinzu_value_pair_frame_t){root, 0, left, right, NULL};
  int equal = 1;
  while (depth && equal && !status) {
    rinzu_value_pair_frame_t *frame = &stack[depth - 1];
    const rinzu_value_type_t *type = &schema->types[frame->type];
    left = frame->left;
    right = frame->right;
    if (type->kind == RINZU_VALUE_SCALAR) {
      equal = rinzu_scalar_compare(type->scalar_kind, 0, left->bits, right->bits);
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_UNIT) {
      depth -= 1;
      continue;
    }
    if (type->kind == RINZU_VALUE_STRING || type->kind == RINZU_VALUE_BYTES ||
        type->kind == RINZU_VALUE_DYNAMIC_BYTES) {
      equal = left->length == right->length &&
              (!left->length || !memcmp(left->bytes, right->bytes, left->length));
      depth -= 1;
      continue;
    }
    if (left->count != right->count ||
        (type->kind == RINZU_VALUE_ENUM && left->variant != right->variant)) {
      equal = 0;
      break;
    }
    if (frame->field == left->count) {
      free(frame->map_index);
      frame->map_index = NULL;
      depth -= 1;
      continue;
    }
    size_t child_type;
    const rinzu_value_t *a, *b;
    if (type->kind == RINZU_VALUE_MAP) {
      if (right->count > 1 && !frame->map_index) {
        frame->map_index = rinzu_value_map_index_new(right->count);
        for (size_t index = 0; index < right->count; ++index) {
          uint64_t hash;
          status = rinzu_value_key_hash(schema,type->key,&right->entries[index].key,&hash);
          if (status)
            break;
          size_t bucket = (size_t)hash & (frame->map_index->capacity - 1);
          while (frame->map_index->buckets[bucket].entry_plus_one)
            bucket = (bucket + 1) & (frame->map_index->capacity - 1);
          frame->map_index->buckets[bucket] = (rinzu_value_map_bucket_t){hash,index + 1};
        }
        if (status)
          break;
      }
      const rinzu_value_pair_t *entry = &left->entries[frame->field];
      size_t matched = SIZE_MAX;
      if (right->count == 1) {
        int same = 0;
        status = rinzu_value_equal(schema,type->key,&entry->key,&right->entries[0].key,&same);
        if (same)
          matched = 0;
      } else {
        uint64_t hash;
        status = rinzu_value_key_hash(schema, type->key, &entry->key, &hash);
        if (status)
          break;
        size_t bucket;
        status = rinzu_value_map_probe(schema,type->key,right->entries,frame->map_index,
                                       &entry->key,hash,&bucket,&matched);
      }
      if (status)
        break;
      if (matched == SIZE_MAX) {
        equal = 0;
        break;
      }
      child_type = type->value;
      a = &entry->value;
      b = &right->entries[matched].value;
    } else {
      child_type = type->kind == RINZU_VALUE_LIST
                       ? type->element
                       : (type->kind == RINZU_VALUE_RECORD
                              ? type->fields[frame->field]
                              : type->variants[left->variant].types[frame->field]);
      a = &left->fields[frame->field];
      b = &right->fields[frame->field];
    }
    frame->field += 1;
    /* All equality-enabled schemas are acyclic, so depth fits their type table. */
    if (depth == schema->count)
      rinzu_failure_fatal();
    stack[depth++] = (rinzu_value_pair_frame_t){child_type, 0, a, b, NULL};
  }
  for (size_t index = 0; index < depth; ++index)
    free(stack[index].map_index);
  free(stack);
  if (status)
    return status;
  *destination = equal;
  return RINZU_VALUE_OK;
}

/* The array owns the normalized container; immutable child views remain
   borrowed from the caller until receiver-side materialization finishes. */
static inline int rinzu_value_map(const rinzu_value_schema_t *schema, size_t root,
                                  const rinzu_value_pair_t *entries, size_t count,
                                  rinzu_value_map_t *destination) {
  if (!schema || root >= schema->count || schema->types[root].kind != RINZU_VALUE_MAP ||
      !destination || destination->entries || destination->count || (count && !entries) ||
      count > SIZE_MAX / sizeof(*entries))
    return RINZU_VALUE_TYPE;
  const rinzu_value_type_t *type = &schema->types[root];
  for (size_t index = 0; index < count; ++index) {
    int status = rinzu_value_validate(schema, type->key, &entries[index].key);
    if (status)
      return status;
    status = rinzu_value_validate(schema, type->value, &entries[index].value);
    if (status)
      return status;
  }
  rinzu_value_map_t prepared = {count ? rinzu_failure_allocate(count * sizeof(*entries)) : NULL, 0};
  rinzu_value_map_index_t *seen = count > 1 ? rinzu_value_map_index_new(count) : NULL;
  int status = RINZU_VALUE_OK;
  for (size_t index = 0; index < count; ++index) {
    uint64_t hash;
    status = rinzu_value_key_hash(schema, type->key, &entries[index].key, &hash);
    if (status)
      break;
    size_t existing = SIZE_MAX, bucket = 0;
    if (seen)
      status = rinzu_value_map_probe(schema,type->key,prepared.entries,seen,
                                     &entries[index].key,hash,&bucket,&existing);
    if (status)
      break;
    if (existing != SIZE_MAX)
      prepared.entries[existing].value = entries[index].value;
    else {
      if (seen)
        seen->buckets[bucket] = (rinzu_value_map_bucket_t){hash,prepared.count + 1};
      prepared.entries[prepared.count++] = entries[index];
    }
  }
  free(seen);
  if (status) {
    free(prepared.entries);
    return status;
  }
  *destination = prepared;
  return RINZU_VALUE_OK;
}
static inline void rinzu_value_map_dispose(rinzu_value_map_t *map) {
  free(map->entries);
  *map = (rinzu_value_map_t){0};
}
#endif
