#include "rinzu/json.h"

#include <limits.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

#if defined(__aarch64__) && !defined(RZ_JSON_SCALAR_ONLY)
#include <arm_neon.h>
#endif

#include "rinzu/key.h"
#include "rinzu/native_fatal.h"
#include "rinzu/resource_counters.h"

#define FFC_IMPL
#include "rinzu/vendor/ffc.h"
#define d2s_buffered_n rinzu_ryu_d2s_buffered_n
#define d2s_buffered rinzu_ryu_d2s_buffered
#define d2s rinzu_ryu_d2s
#include "rinzu/vendor/ryu/d2s.c"
#undef d2s
#undef d2s_buffered
#undef d2s_buffered_n

enum {
  NODE_NULL,
  NODE_FALSE,
  NODE_TRUE,
  NODE_NUMBER,
  NODE_TEXT,
  NODE_ESCAPED,
  NODE_ARRAY,
  NODE_OBJECT
};
enum { FIRST, NEXT, COLON, VALUE, COMMA };
enum { LEX_IDLE, LEX_STRING, LEX_NUMBER };
enum { NUM_SIGN, NUM_ZERO, NUM_INT, NUM_DOT, NUM_FRAC, NUM_EXP, NUM_EXP_SIGN, NUM_EXP_DIGITS };

#define START_BITS 28
#define START_MASK ((UINT32_C(1) << START_BITS) - 1)
#define INDEX_SORTED (UINT32_C(1) << 31)
#define CONTIGUOUS_ARRAY (UINT32_C(1) << 31)
#define CONTIGUOUS_OBJECT (UINT32_C(1) << 30)
#define EDGE_FLAGS (CONTIGUOUS_ARRAY | CONTIGUOUS_OBJECT)
#define SMALL_OBJECT 8
#define MAX_PROBES 32
#define SCAN_BYTES (64 * 1024)
#define SCAN_WORK (16 * 1024)

typedef struct {
  uint32_t tagged_start, detail;
} Node;
typedef struct {
  uint32_t end, first, count, lookup, depth, nodes;
} Container;
typedef struct {
  uint32_t end, offset, length, scalars;
} Text;
typedef struct {
  uint32_t start, end, offset, length, scalars;
} Token;
typedef struct {
  Token token;
  uint32_t node;
} Key;
typedef struct {
  uint32_t node, first, count, depth;
  uint32_t outer_type, base_type, seen_start, key_start, key_text_start, work_start;
  uint32_t duplicate, duplicate_first, key_work;
  unsigned kind, next, contiguous;
} Frame;

_Static_assert(__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__, "JSON index byte order");
_Static_assert(sizeof(Node) == 8 && sizeof(Container) == 24 && sizeof(Text) == 16,
               "JSON index word layout");

enum {
  INDEX_IDLE,
  INDEX_CLEAR,
  INDEX_HASH,
  INDEX_ORDER,
  INDEX_MERGE,
  INDEX_DUPLICATES,
  INDEX_SAVE
};

typedef struct {
  unsigned phase, swapped;
  uint32_t cursor, capacity, first_slot, hashed, compared, slot, probe;
  uint64_t hash;
  uint32_t width, left, right, middle, end;
  uint32_t group, first, second;
} IndexWork;

struct RinzuJsonParser {
  uint32_t length, position, max_depth, max_nodes, nodes_seen;
  unsigned lex, key, number, integral, root_done, finished;
  uint32_t plain;
  Token token;
  RinzuJsonSchema schema;
  RinzuJsonError error;
  IndexWork index_work;
  RinzuJsonBuffer stack, keys, work, seen;
  RinzuJsonBuffer nodes, containers, texts, edges, hashes, decoded, key_text;
  RinzuJsonBuffer order, scratch, path;
};

static uint32_t count_words(const RinzuJsonBuffer *buffer, size_t size) {
  size_t count = buffer->length / size;
  if (count > UINT32_MAX)
    rinzu_native_fatal(71);
  return (uint32_t)count;
}

static __attribute__((noinline)) void reserve(RinzuJsonBuffer *buffer, size_t needed) {
  size_t capacity = buffer->capacity ? buffer->capacity : 64;
  while (capacity < needed) {
    if (capacity > SIZE_MAX / 2)
      rinzu_native_fatal(71);
    capacity *= 2;
  }
  uintptr_t previous = (uintptr_t)buffer->data;
  unsigned char *data = realloc(buffer->data, capacity);
  if (!data)
    rinzu_native_fatal(71);
  if (previous && (uintptr_t)data != previous)
    buffer->moved += buffer->length;
  rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, capacity - buffer->capacity);
  buffer->data = data;
  buffer->capacity = capacity;
}

static inline void *grow(RinzuJsonBuffer *buffer, size_t additional) {
  size_t needed;
  if (__builtin_add_overflow(buffer->length, additional, &needed))
    rinzu_native_fatal(71);
  if (needed > buffer->capacity)
    reserve(buffer, needed);
  void *at = buffer->data ? buffer->data + buffer->length : NULL;
  buffer->length = needed;
  return at;
}

static void append(RinzuJsonBuffer *buffer, const void *data, size_t length) {
  if (length)
    memcpy(grow(buffer, length), data, length);
}

void rinzu_json_buffer_free(RinzuJsonBuffer *buffer) {
  rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, buffer->capacity);
  free(buffer->data);
  *buffer = (RinzuJsonBuffer){0};
}

static void append_word(RinzuJsonBuffer *buffer, uint32_t word) {
  append(buffer, &word, sizeof word);
}

int rinzu_json_limits(RinzuJsonLimits limits, unsigned bytes_kind, RinzuJsonError *error) {
  const int64_t values[] = {limits.bytes, limits.depth, limits.nodes};
  const int64_t minima[] = {1, 0, 1};
  const int64_t maxima[] = {INT64_C(1) << START_BITS, 4096, INT64_C(1) << 25};
  for (size_t i = 0; i < 3; ++i) {
    if (values[i] < minima[i] || values[i] > maxima[i]) {
      *error = (RinzuJsonError){.code = RZ_JSON_INVALID_LIMIT,
                                .reason = i ? (uint32_t)i : bytes_kind,
                                .supplied = values[i],
                                .minimum = minima[i],
                                .maximum = maxima[i]};
      return 0;
    }
  }
  return 1;
}

static void decode_error(RinzuJsonParser *p, unsigned reason, uint32_t offset) {
  p->error = (RinzuJsonError){
      .code = RZ_JSON_DECODE, .reason = reason, .offset = offset, .first = RZ_JSON_NONE};
}

static void limit_error(RinzuJsonParser *p, unsigned reason, uint32_t limit, uint32_t offset) {
  p->error =
      (RinzuJsonError){.code = RZ_JSON_LIMIT, .reason = reason, .offset = offset, .limit = limit};
}

RinzuJsonParser *rinzu_json_parser_new(size_t length, RinzuJsonLimits limits,
                                       const RinzuJsonSchema *schema) {
  RinzuJsonParser *p = calloc(1, sizeof(*p));
  if (!p)
    rinzu_native_fatal(71);
  rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, sizeof(*p));
  if (schema)
    p->schema = *schema;
  if (rinzu_json_limits(limits, RZ_JSON_INPUT, &p->error)) {
    if (length > (size_t)limits.bytes)
      limit_error(p, RZ_JSON_INPUT, (uint32_t)limits.bytes, (uint32_t)limits.bytes);
    else
      p->length = (uint32_t)length;
    p->max_depth = (uint32_t)limits.depth;
    p->max_nodes = (uint32_t)limits.nodes;
  }
  return p;
}

void rinzu_json_parser_free(RinzuJsonParser *p) {
  RinzuJsonBuffer *buffers[] = {&p->stack,      &p->keys,  &p->work,    &p->seen,   &p->nodes,
                                &p->containers, &p->texts, &p->edges,   &p->hashes, &p->decoded,
                                &p->key_text,   &p->order, &p->scratch, &p->path};
  for (size_t i = 0; i < sizeof buffers / sizeof *buffers; ++i)
    rinzu_json_buffer_free(buffers[i]);
  rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, sizeof(*p));
  free(p);
}

size_t rinzu_json_scratch_bytes(const RinzuJsonParser *p) {
  const RinzuJsonBuffer *buffers[] = {
      &p->stack, &p->keys,   &p->work,    &p->seen,     &p->nodes, &p->containers, &p->texts,
      &p->edges, &p->hashes, &p->decoded, &p->key_text, &p->order, &p->scratch,    &p->path};
  size_t bytes = sizeof(*p);
  for (size_t i = 0; i < sizeof buffers / sizeof *buffers; ++i)
    bytes += buffers[i]->capacity;
  return bytes;
}

const RinzuJsonError *rinzu_json_error(const RinzuJsonParser *p) {
  return &p->error;
}

static inline unsigned public_kind(unsigned kind) {
  static const unsigned kinds[] = {RZ_JSON_NULL, RZ_JSON_BOOLEAN, RZ_JSON_BOOLEAN, RZ_JSON_NUMBER,
                                   RZ_JSON_TEXT, RZ_JSON_TEXT,    RZ_JSON_ARRAY,   RZ_JSON_OBJECT};
  return kinds[kind];
}

static inline unsigned utf8_width(const unsigned char *bytes, uint32_t n) {
  unsigned lead = bytes[0];
  unsigned length = lead < 0x80    ? 1
                    : lead < 0xc2  ? 0
                    : lead < 0xe0  ? 2
                    : lead < 0xf0  ? 3
                    : lead <= 0xf4 ? 4
                                   : 0;
  if (length < 2 || length > n)
    return length == 1 ? 1 : 0;
  unsigned b = bytes[1];
  if (b < 0x80 || b > 0xbf || (lead == 0xe0 && b < 0xa0) || (lead == 0xed && b >= 0xa0) ||
      (lead == 0xf0 && b < 0x90) || (lead == 0xf4 && b >= 0x90))
    return 0;
  for (unsigned i = 2; i < length; ++i)
    if (bytes[i] < 0x80 || bytes[i] > 0xbf)
      return 0;
  return length;
}

static inline void unexpected(RinzuJsonParser *p, const unsigned char *bytes, unsigned reason) {
  uint32_t pos = p->position;
  if (pos == p->length)
    reason = RZ_JSON_END;
  else if (reason != RZ_JSON_TRAILING && bytes[pos] >= 0x80 &&
           !utf8_width(bytes + pos, p->length - pos))
    reason = RZ_JSON_UTF8;
  decode_error(p, reason, pos);
}

static inline int is_space(unsigned char c) {
  return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

static inline uint32_t skip_space(const unsigned char *bytes, uint32_t pos, uint32_t end) {
#if defined(__aarch64__) && !defined(RZ_JSON_SCALAR_ONLY)
  if (end - pos >= 16 && is_space(bytes[pos])) {
    for (; end - pos >= 16; pos += 16) {
      uint8x16_t block = vld1q_u8(bytes + pos);
      uint8x16_t space = vceqq_u8(block, vdupq_n_u8(' '));
      space = vorrq_u8(space, vceqq_u8(block, vdupq_n_u8('\t')));
      space = vorrq_u8(space, vceqq_u8(block, vdupq_n_u8('\n')));
      space = vorrq_u8(space, vceqq_u8(block, vdupq_n_u8('\r')));
      if (vminvq_u8(space) != 255)
        break;
    }
  }
#endif
  while (pos < end && is_space(bytes[pos]))
    ++pos;
  return pos;
}

static inline uint32_t plain_string(const unsigned char *bytes, uint32_t pos, uint32_t end) {
  /* Most keys and short strings fit in two words. Subtraction can mark an
     extra byte after a match through borrow, so only the first mark is used. */
  uint32_t prefix = end - pos < 16 ? end : pos + 16;
  for (; prefix - pos >= 8; pos += 8) {
    uint64_t word;
    memcpy(&word, bytes + pos, sizeof word);
    uint64_t quotes = word ^ UINT64_C(0x2222222222222222);
    uint64_t escapes = word ^ UINT64_C(0x5c5c5c5c5c5c5c5c);
    uint64_t special =
        (word | (word - UINT64_C(0x2020202020202020)) | (quotes - UINT64_C(0x0101010101010101)) |
         (escapes - UINT64_C(0x0101010101010101))) &
        UINT64_C(0x8080808080808080);
    if (special)
      return pos + (uint32_t)(__builtin_ctzll(special) >> 3);
  }
#if defined(__aarch64__) && !defined(RZ_JSON_SCALAR_ONLY)
  for (; end - pos >= 16; pos += 16) {
    uint8x16_t block = vld1q_u8(bytes + pos);
    uint8x16_t special =
        vorrq_u8(vcltq_u8(block, vdupq_n_u8(0x20)), vcgeq_u8(block, vdupq_n_u8(0x80)));
    special = vorrq_u8(special, vceqq_u8(block, vdupq_n_u8('"')));
    special = vorrq_u8(special, vceqq_u8(block, vdupq_n_u8('\\')));
    uint8x8_t nibbles = vshrn_n_u16(vreinterpretq_u16_u8(special), 4);
    uint64_t mask = vget_lane_u64(vreinterpret_u64_u8(nibbles), 0);
    if (mask)
      return pos + (uint32_t)(__builtin_ctzll(mask) >> 2);
  }
#endif
  for (; pos < end; ++pos) {
    unsigned c = bytes[pos];
    if (c < 0x20 || c >= 0x80 || c == '"' || c == '\\')
      break;
  }
  return pos;
}

static unsigned encode_scalar(unsigned scalar, unsigned char out[4]) {
  if (scalar <= 0x7f) {
    out[0] = (unsigned char)scalar;
    return 1;
  }
  unsigned count = scalar < 0x800 ? 2 : scalar < 0x10000 ? 3 : 4;
  unsigned lead = count == 2 ? 0xc0 : count == 3 ? 0xe0 : 0xf0;
  for (unsigned i = count - 1; i; --i) {
    out[i] = (unsigned char)(0x80 | (scalar & 0x3f));
    scalar >>= 6;
  }
  out[0] = (unsigned char)(lead | scalar);
  return count;
}

static int hex_escape(RinzuJsonParser *p, const unsigned char *bytes, uint32_t pos,
                      unsigned *result) {
  unsigned value = 0;
  for (unsigned i = 0; i < 4; ++i, ++pos) {
    if (pos == p->length) {
      decode_error(p, RZ_JSON_END, pos);
      return 0;
    }
    unsigned c = bytes[pos], digit;
    if (c >= '0' && c <= '9')
      digit = c - '0';
    else if ((c | 32) >= 'a' && (c | 32) <= 'f')
      digit = (c | 32) - 'a' + 10;
    else {
      decode_error(p, RZ_JSON_ESCAPE, pos);
      return 0;
    }
    value = value * 16 + digit;
  }
  *result = value;
  return 1;
}

static int string_escape(RinzuJsonParser *p, const unsigned char *bytes, RinzuJsonBuffer *decoded) {
  uint32_t start = p->position;
  uint32_t pos = start + 1;
  if (pos == p->length) {
    decode_error(p, RZ_JSON_END, pos);
    return 0;
  }
  unsigned scalar = bytes[pos++];
  switch (scalar) {
  case '"':
  case '\\':
  case '/':
    break;
  case 'b':
    scalar = '\b';
    break;
  case 'f':
    scalar = '\f';
    break;
  case 'n':
    scalar = '\n';
    break;
  case 'r':
    scalar = '\r';
    break;
  case 't':
    scalar = '\t';
    break;
  case 'u': {
    if (!hex_escape(p, bytes, pos, &scalar))
      return 0;
    pos += 4;
    if (scalar >= 0xd800 && scalar <= 0xdbff) {
      unsigned low;
      if (p->length - pos < 2 || bytes[pos] != '\\' || bytes[pos + 1] != 'u') {
        decode_error(p, RZ_JSON_SURROGATE, start);
        return 0;
      }
      if (!hex_escape(p, bytes, pos + 2, &low))
        return 0;
      if (low < 0xdc00 || low > 0xdfff) {
        decode_error(p, RZ_JSON_SURROGATE, start);
        return 0;
      }
      scalar = 0x10000 + ((scalar - 0xd800) << 10) + low - 0xdc00;
      pos += 6;
    } else if (scalar >= 0xdc00 && scalar <= 0xdfff) {
      decode_error(p, RZ_JSON_SURROGATE, start);
      return 0;
    }
    break;
  }
  default:
    decode_error(p, RZ_JSON_ESCAPE, pos - 1);
    return 0;
  }
  if (p->token.offset == RZ_JSON_NONE)
    p->token.offset = count_words(decoded, 1);
  append(decoded, bytes + p->plain, start - p->plain);
  unsigned char escaped[4];
  size_t length = encode_scalar(scalar, escaped);
  append(decoded, escaped, length);
  p->plain = p->position = pos;
  ++p->token.scalars;
  return 1;
}

static void begin_string(RinzuJsonParser *p, unsigned key) {
  p->key = key;
  p->token = (Token){.start = p->position++, .offset = RZ_JSON_NONE};
  p->plain = p->position;
  p->lex = LEX_STRING;
}

static int scan_string(RinzuJsonParser *p, const unsigned char *bytes, uint32_t end) {
  RinzuJsonBuffer *decoded = p->key && p->schema.types ? &p->key_text : &p->decoded;
  while (p->position < end) {
    uint32_t pos = plain_string(bytes, p->position, end);
    p->token.scalars += pos - p->position;
    p->position = pos;
    if (pos == end)
      break;
    unsigned c = bytes[pos];
    if (c == '"') {
      if (p->token.offset != RZ_JSON_NONE) {
        append(decoded, bytes + p->plain, pos - p->plain);
        p->token.length = count_words(decoded, 1) - p->token.offset;
      } else {
        p->token.length = pos - p->token.start - 1;
      }
      p->token.end = ++p->position;
      p->lex = LEX_IDLE;
      return 1;
    }
    if (c == '\\') {
      if (!string_escape(p, bytes, decoded))
        return 0;
    } else if (c < 0x20) {
      decode_error(p, RZ_JSON_CONTROL, pos);
      return 0;
    } else {
      unsigned width = utf8_width(bytes + pos, p->length - pos);
      if (!width) {
        decode_error(p, RZ_JSON_UTF8, pos);
        return 0;
      }
      ++p->token.scalars;
      p->position += width;
    }
  }
  if (p->position == p->length)
    decode_error(p, RZ_JSON_END, p->length);
  return 0;
}

static inline int digit(unsigned c) {
  return c - '0' <= 9;
}

static void begin_number(RinzuJsonParser *p, const unsigned char *bytes) {
  p->token = (Token){.start = p->position};
  unsigned first = bytes[p->position++];
  p->number = first == '-' ? NUM_SIGN : first == '0' ? NUM_ZERO : NUM_INT;
  p->integral = 1;
  p->lex = LEX_NUMBER;
}

static int scan_number(RinzuJsonParser *p, const unsigned char *bytes, uint32_t end) {
  uint32_t pos = p->position;
  unsigned stage = p->number;
  while (pos < end) {
    unsigned c = bytes[pos];
    switch (stage) {
    case NUM_EXP:
      stage = NUM_EXP_SIGN;
      if (c == '+' || c == '-') {
        ++pos;
        continue;
      }
      break;
    case NUM_ZERO:
    case NUM_INT:
    case NUM_FRAC:
    case NUM_EXP_DIGITS:
      if (digit(c)) {
        if (stage == NUM_ZERO)
          goto invalid;
        do {
          ++pos;
        } while (pos < end && digit(bytes[pos]));
        continue;
      }
      if ((stage == NUM_ZERO || stage == NUM_INT) && c == '.') {
        stage = NUM_DOT;
        p->integral = 0;
        ++pos;
        continue;
      }
      if (stage != NUM_EXP_DIGITS && (c == 'e' || c == 'E')) {
        stage = NUM_EXP;
        p->integral = 0;
        ++pos;
        continue;
      }
      goto complete;
    default:
      break;
    }
    if (!digit(c))
      goto invalid;
    stage = stage == NUM_SIGN  ? (c == '0' ? NUM_ZERO : NUM_INT)
            : stage == NUM_DOT ? NUM_FRAC
                               : NUM_EXP_DIGITS;
    ++pos;
  }
  if (pos == p->length) {
    if (stage == NUM_SIGN || stage == NUM_DOT || stage == NUM_EXP || stage == NUM_EXP_SIGN) {
      decode_error(p, RZ_JSON_END, pos);
      return 0;
    }
    goto complete;
  }
  p->position = pos;
  p->number = stage;
  return 0;

invalid:
  p->position = pos;
  unexpected(p, bytes, RZ_JSON_BAD_NUMBER);
  return 0;
complete:
  p->token.end = p->position = pos;
  p->lex = LEX_IDLE;
  return 1;
}

int rinzu_json_number_valid(RinzuJsonSlice token) {
  if (!token.length || token.length > UINT32_MAX || (token.data[0] != '-' && !digit(token.data[0])))
    return 0;
  RinzuJsonParser parser = {.length = (uint32_t)token.length};
  begin_number(&parser, token.data);
  return scan_number(&parser, token.data, parser.length) && parser.position == parser.length;
}

static int convert_int(RinzuJsonSlice token, unsigned check_syntax, uint64_t *bits) {
  const unsigned char *pos = token.data;
  const unsigned char *end = pos + token.length;
  unsigned negative = *pos == '-';
  pos += negative;
  uint64_t limit = (uint64_t)INT64_MAX + negative, magnitude = 0;
  if (end - pos <= 18) {
    for (; pos < end; ++pos) {
      unsigned d = *pos - (unsigned)'0';
      if (check_syntax && d > 9)
        return RZ_JSON_INTEGER;
      magnitude = magnitude * 10 + d;
    }
    *bits = negative ? UINT64_C(0) - magnitude : magnitude;
    return -1;
  }
  unsigned overflow = 0;
  for (; pos < end; ++pos) {
    unsigned d = *pos - (unsigned)'0';
    if (check_syntax && d > 9)
      return RZ_JSON_INTEGER;
    if (!overflow) {
      if (magnitude > (limit - d) / 10)
        overflow = 1;
      else
        magnitude = magnitude * 10 + d;
    }
  }
  if (overflow)
    return RZ_JSON_RANGE;
  *bits = negative ? UINT64_C(0) - magnitude : magnitude;
  return -1;
}

int rinzu_json_int(RinzuJsonSlice token, uint64_t *bits) {
  return convert_int(token, 1, bits);
}

int rinzu_json_float(RinzuJsonSlice token, uint64_t *bits) {
  const char *start = (const char *)token.data, *end = start + token.length;
  double value;
  ffc_result result =
      ffc_from_chars_double_options(start, end, &value, (ffc_parse_options){FFC_PRESET_JSON, '.'});
  if (result.ptr != end || result.outcome == FFC_OUTCOME_INVALID_INPUT)
    return RZ_JSON_SYNTAX;
  /* ffc reports underflow as out-of-range as well. Its rounded signed zero
     is part of the Rinzu result, including for a nonzero input. */
  if (!isfinite(value))
    return RZ_JSON_RANGE;
  memcpy(bits, &value, sizeof value);
  return -1;
}

size_t rinzu_json_format_int(uint64_t bits, unsigned char out[32]) {
  unsigned negative = bits >> 63;
  uint64_t value = negative ? UINT64_C(0) - bits : bits;
  unsigned char reverse[20];
  size_t length = 0;
  do {
    reverse[length++] = (unsigned char)('0' + value % 10);
    value /= 10;
  } while (value);
  size_t written = 0;
  if (negative)
    out[written++] = '-';
  while (length)
    out[written++] = reverse[--length];
  return written;
}

size_t rinzu_json_format_float(uint64_t bits, unsigned char out[32]) {
  double value;
  memcpy(&value, &bits, sizeof value);
  if (!isfinite(value))
    return 0;
  size_t written = 0;
  if (signbit(value)) {
    out[written++] = '-';
    value = -value;
  }
  if (value == 0) {
    memcpy(out + written, "0.0", 3);
    return written + 3;
  }
  char raw[32];
  size_t length = (size_t)rinzu_ryu_d2s_buffered_n(value, raw);
  unsigned char digits[17];
  size_t digits_count = 0, i = 0;
  for (; i < length && raw[i] != 'E'; ++i)
    if (raw[i] != '.')
      digits[digits_count++] = (unsigned char)raw[i];
  ++i;
  int negative = raw[i] == '-';
  i += (size_t)negative;
  int exponent = 0;
  for (; i < length; ++i)
    exponent = exponent * 10 + raw[i] - '0';
  if (negative)
    exponent = -exponent;
  if (exponent >= -6 && exponent <= 20) {
    int decimal = exponent + 1;
    if (decimal <= 0) {
      out[written++] = '0';
      out[written++] = '.';
      for (; decimal < 0; ++decimal)
        out[written++] = '0';
      memcpy(out + written, digits, digits_count);
      return written + digits_count;
    }
    for (i = 0; i < (size_t)decimal; ++i)
      out[written++] = i < digits_count ? digits[i] : '0';
    out[written++] = '.';
    if (i == digits_count || i > digits_count) {
      out[written++] = '0';
    } else {
      memcpy(out + written, digits + i, digits_count - i);
      written += digits_count - i;
    }
  } else {
    out[written++] = digits[0];
    if (digits_count > 1) {
      out[written++] = '.';
      memcpy(out + written, digits + 1, digits_count - 1);
      written += digits_count - 1;
    }
    out[written++] = 'e';
    if (exponent > 0)
      out[written++] = '+';
    unsigned char number[32];
    size_t count = rinzu_json_format_int((uint64_t)(int64_t)exponent, number);
    memcpy(out + written, number, count);
    written += count;
  }
  return written;
}

static RinzuJsonSlice token_text(Token token, const unsigned char *bytes,
                                 const RinzuJsonBuffer *decoded) {
  return (RinzuJsonSlice){token.offset == RZ_JSON_NONE ? bytes + token.start + 1
                                                       : decoded->data + token.offset,
                          token.length};
}

static Frame *top(RinzuJsonParser *p) {
  return (Frame *)(p->stack.data + p->stack.length) - 1;
}

static inline const RinzuJsonBuffer *key_text(const RinzuJsonParser *p) {
  return p->schema.types ? &p->key_text : &p->decoded;
}

static inline const Key *frame_keys(const RinzuJsonParser *p, const Frame *frame) {
  return (const Key *)p->keys.data + frame->key_start;
}

static int compare_key(RinzuJsonSlice a, RinzuJsonSlice b) {
  if (a.length != b.length)
    return a.length < b.length ? -1 : 1;
  return a.length ? memcmp(a.data, b.data, a.length) : 0;
}

static int compare_key_step(IndexWork *state, uint32_t *work, RinzuJsonSlice a, RinzuJsonSlice b,
                            int *order) {
  if (*work == SCAN_WORK)
    return 0;
  if (a.length != b.length || !a.length) {
    *order = (a.length > b.length) - (a.length < b.length);
    ++*work;
    return 1;
  }
  size_t remaining = a.length - state->compared;
  size_t available = (SCAN_WORK - *work) * 4;
  size_t count = remaining < available ? remaining : available;
  *order = memcmp(a.data + state->compared, b.data + state->compared, count);
  *work += (uint32_t)((count + 3) / 4);
  if (!*order && count < remaining) {
    state->compared += (uint32_t)count;
    return 0;
  }
  state->compared = 0;
  return 1;
}

static int compare_field(RinzuJsonSlice a, RinzuJsonSlice b) {
  size_t n = a.length < b.length ? a.length : b.length;
  int cmp = n ? memcmp(a.data, b.data, n) : 0;
  return cmp ? cmp : (a.length > b.length) - (a.length < b.length);
}

static uint64_t hash_key(RinzuJsonSlice text) {
  rinzu_key_hasher_t hash = rinzu_key_init();
  rinzu_key_write(&hash, text.data, text.length);
#ifdef RZ_JSON_TEST_COLLISIONS
  return 0;
#else
  return hash.hash ^ (hash.hash >> 32);
#endif
}

static RinzuJsonSlice object_key(const RinzuJsonParser *p, const unsigned char *bytes,
                                 const Key *keys, uint32_t index) {
  return token_text(keys[index].token, bytes, key_text(p));
}

static int duplicate(RinzuJsonParser *p, const Key *keys, uint32_t second, uint32_t first) {
  decode_error(p, RZ_JSON_DUPLICATE, keys[second].token.start);
  p->error.first = keys[first].token.start;
  return 0;
}

static uint32_t hash_capacity(uint32_t count) {
  /* Round ceil(count / .75) up to a power of two. Node limits leave
     headroom for these multiplications and the shift. */
  return UINT32_C(1) << (32 - __builtin_clz((count * 4 - 1) / 3));
}

/* Wide objects may span several polls after their closing delimiter. State
   records offsets and work cursors only, so input can move between steps.
   Stable merge order makes the earliest second occurrence deterministic. */
static int index_keys(RinzuJsonParser *p, const unsigned char *bytes, const Frame *frame,
                      uint32_t *lookup, uint32_t *work) {
  uint32_t count = frame->count;
  *lookup = 0;
  if (count < 2)
    return 1;
  const Key *keys = frame_keys(p, frame);
  IndexWork *state = &p->index_work;
  if (count <= SMALL_OBJECT) {
    uint32_t budget = count * (count - 1) / 2 * frame->key_work;
    if (!state->cursor && budget <= SCAN_WORK - *work) {
      *work += budget;
      for (uint32_t i = 1; i < count; ++i) {
        RinzuJsonSlice key = object_key(p, bytes, keys, i);
        for (uint32_t j = 0; j < i; ++j)
          if (!compare_key(key, object_key(p, bytes, keys, j)))
            return duplicate(p, keys, i, j);
      }
      return 1;
    }
    if (!state->cursor)
      state->cursor = 1;
    for (uint32_t i = state->cursor; i < count; i = ++state->cursor) {
      RinzuJsonSlice key = object_key(p, bytes, keys, i);
      for (uint32_t j = state->probe; j < i; j = ++state->probe) {
        int order;
        if (!compare_key_step(state, work, key, object_key(p, bytes, keys, j), &order))
          return 0;
        if (!order)
          return duplicate(p, keys, i, j);
      }
      state->probe = 0;
    }
    state->cursor = 0;
    return 1;
  }

  RinzuJsonBuffer *buffer = p->schema.types ? &p->order : &p->hashes;
  if (state->phase == INDEX_IDLE) {
    if (p->schema.types)
      buffer->length = 0;
    state->capacity = hash_capacity(count);
    state->first_slot = count_words(buffer, sizeof(uint32_t));
    (void)grow(buffer, (size_t)state->capacity * sizeof(uint32_t));
    state->phase = INDEX_CLEAR;
  }
  if (state->phase == INDEX_CLEAR) {
    uint32_t available = SCAN_WORK - *work;
    uint32_t n = state->capacity - state->cursor;
    if (n > available)
      n = available;
    uint32_t *table = (uint32_t *)buffer->data + state->first_slot;
    memset(table + state->cursor, 0xff, (size_t)n * sizeof(*table));
    state->cursor += n;
    *work += n;
    if (state->cursor < state->capacity)
      return 0;
    state->cursor = 0;
    state->phase = INDEX_HASH;
  }
  if (state->phase == INDEX_HASH) {
    uint32_t *table = (uint32_t *)buffer->data + state->first_slot;
    uint32_t mask = state->capacity - 1;
    while (state->cursor < count && *work < SCAN_WORK) {
      uint32_t i = state->cursor;
      RinzuJsonSlice key = object_key(p, bytes, keys, i);
      if (!state->probe && !state->compared) {
        rinzu_key_hasher_t hash =
            state->hashed ? (rinzu_key_hasher_t){state->hash} : rinzu_key_init();
        size_t n = key.length - state->hashed;
        size_t available = (SCAN_WORK - *work) * 4;
        if (n > available)
          n = available;
        rinzu_key_write(&hash, key.data + state->hashed, n);
        state->hash = hash.hash;
        state->hashed += (uint32_t)n;
        *work += (uint32_t)((n + 3) / 4);
        if (state->hashed < key.length)
          return 0;
#ifdef RZ_JSON_TEST_COLLISIONS
        state->slot = 0;
#else
        state->slot = (uint32_t)(hash.hash ^ (hash.hash >> 32)) & mask;
#endif
      }
      for (; state->probe < MAX_PROBES; ++state->probe, state->slot = (state->slot + 1) & mask) {
        if (*work == SCAN_WORK)
          return 0;
        uint32_t found = table[state->slot];
        if (found == RZ_JSON_NONE) {
          table[state->slot] = i;
          ++*work;
          break;
        }
        int order;
        if (!compare_key_step(state, work, key, object_key(p, bytes, keys, found), &order))
          return 0;
        if (!order)
          return duplicate(p, keys, i, found);
      }
      if (state->probe == MAX_PROBES) {
        buffer->length = (size_t)state->first_slot * sizeof(*table);
        p->order.length = p->scratch.length = 0;
        (void)grow(&p->order, (size_t)count * sizeof(*table));
        (void)grow(&p->scratch, (size_t)count * sizeof(*table));
        state->cursor = 0;
        state->phase = INDEX_ORDER;
        break;
      }
      state->hashed = state->probe = 0;
      ++state->cursor;
    }
    if (state->phase == INDEX_HASH) {
      if (state->cursor < count)
        return 0;
      *lookup = state->first_slot;
      *state = (IndexWork){0};
      return 1;
    }
  }

  uint32_t *order = (uint32_t *)p->order.data;
  uint32_t *scratch = (uint32_t *)p->scratch.data;
  if (state->phase == INDEX_ORDER) {
    while (state->cursor < count && *work < SCAN_WORK) {
      order[state->cursor] = state->cursor;
      ++state->cursor;
      ++*work;
    }
    if (state->cursor < count)
      return 0;
    state->cursor = 0;
    state->width = 1;
    state->phase = INDEX_MERGE;
  }
  if (state->phase == INDEX_MERGE) {
    while (state->width < count && *work < SCAN_WORK) {
      uint32_t i = state->cursor;
      if (i % (2 * state->width) == 0) {
        state->left = i;
        uint32_t middle = i + state->width, end = middle + state->width;
        state->right = state->middle = middle < count ? middle : count;
        state->end = end < count ? end : count;
      }
      const uint32_t *input = state->swapped ? scratch : order;
      uint32_t *output = state->swapped ? order : scratch;
      int cmp = 0;
      if (state->left == state->middle || state->right == state->end)
        ++*work;
      else if (!compare_key_step(state, work, object_key(p, bytes, keys, input[state->left]),
                                 object_key(p, bytes, keys, input[state->right]), &cmp))
        return 0;
      if (state->right == state->end || (state->left < state->middle && cmp <= 0))
        output[i] = input[state->left++];
      else
        output[i] = input[state->right++];
      if (++state->cursor == count) {
        state->swapped ^= 1;
        state->cursor = 0;
        state->width *= 2;
      }
    }
    if (state->width < count)
      return 0;
    state->cursor = 1;
    state->second = RZ_JSON_NONE;
    state->phase = INDEX_DUPLICATES;
  }
  const uint32_t *sorted = state->swapped ? scratch : order;
  if (state->phase == INDEX_DUPLICATES) {
    while (state->cursor < count) {
      uint32_t i = state->cursor;
      int cmp;
      if (!compare_key_step(state, work, object_key(p, bytes, keys, sorted[state->group]),
                            object_key(p, bytes, keys, sorted[i]), &cmp))
        return 0;
      if (cmp) {
        state->group = i;
      } else if (sorted[i] < state->second) {
        state->first = sorted[state->group];
        state->second = sorted[i];
      }
      ++state->cursor;
    }
    if (state->second != RZ_JSON_NONE)
      return duplicate(p, keys, state->second, state->first);
    state->phase = INDEX_SAVE;
    state->cursor = 0;
    state->first_slot = count_words(&p->hashes, sizeof(uint32_t));
  }
  if (!p->schema.types) {
    uint32_t n = count - state->cursor;
    if (n > SCAN_WORK - *work)
      n = SCAN_WORK - *work;
    if (n)
      append(&p->hashes, sorted + state->cursor, (size_t)n * sizeof(*sorted));
    state->cursor += n;
    *work += n;
    if (state->cursor < count)
      return 0;
    *lookup = INDEX_SORTED | state->first_slot;
  }
  *state = (IndexWork){0};
  return 1;
}

static void pointer_part(RinzuJsonBuffer *path, RinzuJsonSlice key) {
  append(path, "/", 1);
  if (!key.data) {
    if (key.length)
      rinzu_native_fatal(86);
    return;
  }
  for (size_t i = 0, start = 0;; ++i) {
    if (i == key.length) {
      if (i != start)
        append(path, key.data + start, i - start);
      break;
    }
    if (key.data[i] == '~' || key.data[i] == '/') {
      append(path, key.data + start, i - start);
      append(path, key.data[i] == '~' ? "~0" : "~1", 2);
      start = i + 1;
    }
  }
}

static void schema_error(RinzuJsonParser *p, const unsigned char *bytes, unsigned reason,
                         uint32_t offset, RinzuJsonSlice key, unsigned expected, unsigned actual) {
  uint32_t depth = count_words(&p->stack, sizeof(Frame));
  const Frame *frames = (const Frame *)p->stack.data;
  p->path.length = 0;
  for (uint32_t i = 0; i < depth - (key.data != NULL); ++i) {
    const Frame *frame = &frames[i];
    if (frame->kind == NODE_ARRAY) {
      unsigned char digits[32];
      size_t length = rinzu_json_format_int(frame->count, digits);
      pointer_part(&p->path, (RinzuJsonSlice){digits, length});
    } else {
      pointer_part(&p->path, object_key(p, bytes, frame_keys(p, frame), frame->count));
    }
  }
  if (key.data)
    pointer_part(&p->path, key);
  p->error = (RinzuJsonError){.code = RZ_JSON_SCHEMA,
                              .reason = reason,
                              .offset = offset,
                              .expected = expected,
                              .actual = actual,
                              .key = key,
                              .path = {p->path.data, p->path.length}};
}

static RinzuJsonSlice field_name(const RinzuJsonSchema *schema, const RinzuJsonField *field) {
  return (RinzuJsonSlice){schema->names + field->name, field->length};
}

static uint32_t record_field(const RinzuJsonSchema *schema, const RinzuJsonType *type,
                             RinzuJsonSlice key) {
  const RinzuJsonField *fields = schema->fields + type->first;
  uint32_t count = type->count;
  if (count <= SMALL_OBJECT) {
    for (uint32_t i = 0; i < count; ++i)
      if (!compare_key(key, field_name(schema, &fields[i])))
        return i;
  } else {
    uint32_t lo = 0, hi = count;
    while (lo < hi) {
      uint32_t middle = lo + (hi - lo) / 2;
      int cmp = compare_field(key, field_name(schema, &fields[middle]));
      if (!cmp)
        return middle;
      if (cmp < 0)
        hi = middle;
      else
        lo = middle + 1;
    }
  }
  return RZ_JSON_NONE;
}

static uint32_t current_type(RinzuJsonParser *p) {
  if (!p->stack.length)
    return p->schema.root;
  const Frame *frame = top(p);
  const RinzuJsonType *type = &p->schema.types[frame->base_type];
  return type->kind == RZ_JSON_RECORD_TY ? frame->first : type->child;
}

static uint32_t base_type(const RinzuJsonSchema *schema, uint32_t outer, unsigned actual,
                          unsigned *expected) {
  uint32_t type = outer;
  while (schema->types[type].kind == RZ_JSON_OPTION_TY) {
    if (actual == RZ_JSON_NULL) {
      *expected = RZ_JSON_NULL;
      return type;
    }
    type = schema->types[type].child;
  }
  switch (schema->types[type].kind) {
  case RZ_JSON_BOOL_TY:
    *expected = RZ_JSON_BOOLEAN;
    break;
  case RZ_JSON_INT_TY:
  case RZ_JSON_FLOAT_TY:
    *expected = RZ_JSON_NUMBER;
    break;
  case RZ_JSON_STRING_TY:
    *expected = RZ_JSON_TEXT;
    break;
  case RZ_JSON_LIST_TY:
    *expected = RZ_JSON_ARRAY;
    break;
  case RZ_JSON_MAP_TY:
  case RZ_JSON_RECORD_TY:
    *expected = RZ_JSON_OBJECT;
    break;
  default:
    rinzu_native_fatal(86);
  }
  return type;
}

static void emit(RinzuJsonBatch *batch, RinzuJsonEvent event) {
  if (!batch)
    rinzu_native_fatal(86);
  batch->events[batch->count++] = event;
}

static int check_node(RinzuJsonParser *p, unsigned container) {
  if (container && count_words(&p->stack, sizeof(Frame)) == p->max_depth) {
    limit_error(p, RZ_JSON_DEPTH, p->max_depth, p->position);
    return 0;
  }
  if (p->nodes_seen == p->max_nodes) {
    limit_error(p, RZ_JSON_NODES, p->max_nodes, p->position);
    return 0;
  }
  ++p->nodes_seen;
  return 1;
}

static uint32_t add_node(RinzuJsonParser *p, uint32_t start, unsigned kind, uint32_t detail) {
  uint32_t node = count_words(&p->nodes, sizeof(Node));
  *(Node *)grow(&p->nodes, sizeof(Node)) = (Node){start | ((uint32_t)kind << START_BITS), detail};
  return node;
}

static void add_array_child(RinzuJsonParser *p, uint32_t node) {
  if (!p->stack.length)
    return;
  Frame *frame = top(p);
  if (frame->kind != NODE_ARRAY)
    return;
  if (!frame->count)
    frame->first = node;
  if (frame->contiguous && node == frame->first + frame->count)
    return;
  if (frame->contiguous) {
    uint32_t *work = grow(&p->work, (size_t)frame->count * sizeof(uint32_t));
    for (uint32_t i = 0; i < frame->count; ++i)
      work[i] = frame->first + i;
    frame->contiguous = 0;
  }
  append_word(&p->work, node);
}

static uint32_t add_text(RinzuJsonParser *p) {
  Token token = p->token;
  if (token.offset == RZ_JSON_NONE)
    return add_node(p, token.start, NODE_TEXT, token.end);
  Text text = {token.end, token.offset, token.length, token.scalars};
  uint32_t index = count_words(&p->texts, sizeof(Text));
  append(&p->texts, &text, sizeof text);
  return add_node(p, token.start, NODE_ESCAPED, index);
}

static void finish_value(RinzuJsonParser *p) {
  if (p->stack.length) {
    Frame *frame = top(p);
    ++frame->count;
    frame->next = COMMA;
  } else {
    p->root_done = 1;
  }
}

static int scalar(RinzuJsonParser *p, const unsigned char *bytes, unsigned node_kind,
                  RinzuJsonBatch *batch) {
  Token token = p->token;
  unsigned kind = public_kind(node_kind);
  if (p->schema.types) {
    uint32_t outer = current_type(p);
    unsigned expected;
    uint32_t base = base_type(&p->schema, outer, kind, &expected);
    if (expected != kind) {
      schema_error(p, bytes, RZ_JSON_WRONG_KIND, token.start, (RinzuJsonSlice){0}, expected, kind);
      return 0;
    }
    RinzuJsonEvent event = {.event = RZ_JSON_SCALAR, .type = outer, .kind = kind};
    if (kind == RZ_JSON_NUMBER) {
      unsigned integer = p->schema.types[base].kind == RZ_JSON_INT_TY;
      RinzuJsonSlice number = {bytes + token.start, token.end - token.start};
      int status = integer ? (p->integral ? convert_int(number, 0, &event.bits) : RZ_JSON_INTEGER)
                           : rinzu_json_float(number, &event.bits);
      if (status >= 0) {
        unsigned reason = integer
                              ? (status == RZ_JSON_INTEGER ? RZ_JSON_EXPECT_INT : RZ_JSON_INT_RANGE)
                              : RZ_JSON_FLOAT_RANGE;
        schema_error(p, bytes, reason, token.start, (RinzuJsonSlice){0}, 0, 0);
        return 0;
      }
    } else if (kind == RZ_JSON_BOOLEAN) {
      event.bits = node_kind == NODE_TRUE;
    } else if (kind == RZ_JSON_TEXT) {
      event.start = token.offset == RZ_JSON_NONE ? token.start + 1 : token.offset;
      event.length = token.length;
      event.scalars = token.scalars;
      event.decoded = token.offset != RZ_JSON_NONE;
    }
    emit(batch, event);
  } else {
    uint32_t node =
        kind == RZ_JSON_TEXT ? add_text(p) : add_node(p, token.start, node_kind, token.end);
    add_array_child(p, node);
  }
  finish_value(p);
  return 1;
}

static void finish_key(RinzuJsonParser *p) {
  Key key = {.token = p->token, .node = p->schema.types ? 0 : add_text(p)};
  append(&p->keys, &key, sizeof key);
  Frame *frame = top(p);
  uint32_t cost = (p->token.length + 3) / 4;
  if (frame->key_work < cost)
    frame->key_work = cost;
  frame->next = COLON;
}

static int key_colon(RinzuJsonParser *p, const unsigned char *bytes, RinzuJsonBatch *batch) {
  Frame *frame = top(p);
  if (bytes[p->position] != ':') {
    unexpected(p, bytes, RZ_JSON_TOKEN);
    return 0;
  }
  ++p->position;
  if (p->schema.types) {
    const RinzuJsonType *type = &p->schema.types[frame->base_type];
    Token token = frame_keys(p, frame)[frame->count].token;
    RinzuJsonSlice key = token_text(token, bytes, &p->key_text);
    RinzuJsonEvent event = {.event = RZ_JSON_KEY, .field = RZ_JSON_NONE};
    if (type->kind == RZ_JSON_RECORD_TY) {
      const RinzuJsonField *fields = p->schema.fields + type->first;
      uint32_t index = record_field(&p->schema, type, key);
      if (index == RZ_JSON_NONE) {
        schema_error(p, bytes, RZ_JSON_UNKNOWN, token.start, key, 0, 0);
        return 0;
      }
      uint32_t *seen = (uint32_t *)p->seen.data + frame->seen_start;
      if (!seen[index]) {
        seen[index] = token.start + 1;
      } else if (frame->duplicate == RZ_JSON_NONE) {
        frame->duplicate = token.start;
        frame->duplicate_first = seen[index] - 1;
      }
      frame->first = fields[index].type;
      event.field = index;
    } else {
      event.length = token.length;
      event.scalars = token.scalars;
      event.decoded = token.offset != RZ_JSON_NONE;
      event.start = token.start + 1;
      if (event.decoded) {
        event.start = count_words(&p->decoded, 1);
        append(&p->decoded, key.data, key.length);
      }
    }
    emit(batch, event);
  }
  frame->next = VALUE;
  return 1;
}

static int open_container(RinzuJsonParser *p, const unsigned char *bytes, unsigned kind,
                          RinzuJsonBatch *batch) {
  Frame frame = {.kind = kind,
                 .contiguous = 1,
                 .depth = 1,
                 .key_work = 1,
                 .duplicate = RZ_JSON_NONE,
                 .key_start = count_words(&p->keys, sizeof(Key)),
                 .key_text_start = count_words(&p->key_text, 1),
                 .work_start = count_words(&p->work, sizeof(uint32_t))};
  if (p->schema.types) {
    unsigned actual = public_kind(kind), expected;
    frame.outer_type = current_type(p);
    frame.base_type = base_type(&p->schema, frame.outer_type, actual, &expected);
    if (expected != actual) {
      schema_error(p, bytes, RZ_JSON_WRONG_KIND, p->position, (RinzuJsonSlice){0}, expected,
                   actual);
      return 0;
    }
    const RinzuJsonType *type = &p->schema.types[frame.base_type];
    frame.seen_start = count_words(&p->seen, sizeof(uint32_t));
    if (type->kind == RZ_JSON_RECORD_TY && type->count) {
      size_t bytes = (size_t)type->count * sizeof(uint32_t);
      memset(grow(&p->seen, bytes), 0, bytes);
    }
    emit(batch, (RinzuJsonEvent){.event = RZ_JSON_OPEN, .type = frame.outer_type, .kind = actual});
  } else {
    frame.node = add_node(p, p->position, kind, 0);
    add_array_child(p, frame.node);
    /* The parent may have added or expanded its own children just now. */
    frame.work_start = count_words(&p->work, sizeof(uint32_t));
  }
  append(&p->stack, &frame, sizeof frame);
  ++p->position;
  return 1;
}

static int close_container(RinzuJsonParser *p, const unsigned char *bytes, RinzuJsonBatch *batch,
                           uint32_t *work) {
  Frame *frame = top(p);
  uint32_t lookup = 0;
  const RinzuJsonType *type = p->schema.types ? &p->schema.types[frame->base_type] : NULL;
  int record = type && type->kind == RZ_JSON_RECORD_TY;
  if (frame->kind == NODE_OBJECT && !record && !index_keys(p, bytes, frame, &lookup, work))
    return 0;
  if (p->schema.types) {
    if (record) {
      if (frame->duplicate != RZ_JSON_NONE) {
        decode_error(p, RZ_JSON_DUPLICATE, frame->duplicate);
        p->error.first = frame->duplicate_first;
        return 0;
      }
      const uint32_t *seen =
          type->count ? (const uint32_t *)p->seen.data + frame->seen_start : NULL;
      for (uint32_t i = 0; i < type->count; ++i) {
        const RinzuJsonField *field = &p->schema.fields[type->first + i];
        if (!seen[i] && p->schema.types[field->type].kind != RZ_JSON_OPTION_TY) {
          schema_error(p, bytes, RZ_JSON_MISSING, p->position, field_name(&p->schema, field), 0, 0);
          return 0;
        }
      }
    }
    emit(batch, (RinzuJsonEvent){.event = RZ_JSON_CLOSE,
                                 .type = frame->outer_type,
                                 .kind = public_kind(frame->kind)});
    p->seen.length = (size_t)frame->seen_start * sizeof(uint32_t);
    p->key_text.length = frame->key_text_start;
  } else {
    uint32_t first = count_words(&p->edges, sizeof(uint32_t));
    if (frame->kind == NODE_OBJECT) {
      if (frame->count) {
        const Key *keys = frame_keys(p, frame);
        uint32_t i = 1;
        while (i < frame->count && keys[i].node == keys[0].node + i * 2)
          ++i;
        if (i == frame->count) {
          first = keys[0].node | CONTIGUOUS_OBJECT;
        } else {
          uint32_t *edges = grow(&p->edges, (size_t)frame->count * sizeof(uint32_t));
          for (i = 0; i < frame->count; ++i)
            edges[i] = keys[i].node;
        }
      }
    } else if (frame->contiguous) {
      first = frame->first | CONTIGUOUS_ARRAY;
    } else {
      append(&p->edges, (const uint32_t *)p->work.data + frame->work_start,
             (size_t)frame->count * sizeof(uint32_t));
    }
    Container c = {p->position + 1, first,
                   frame->count,    lookup,
                   frame->depth,    count_words(&p->nodes, sizeof(Node)) - frame->node};
    ((Node *)p->nodes.data)[frame->node].detail = count_words(&p->containers, sizeof(Container));
    append(&p->containers, &c, sizeof c);
    p->work.length = (size_t)frame->work_start * sizeof(uint32_t);
  }
  uint32_t depth = frame->depth;
  p->keys.length = (size_t)frame->key_start * sizeof(Key);
  p->stack.length -= sizeof(Frame);
  if (p->stack.length && top(p)->depth <= depth)
    top(p)->depth = depth + 1;
  ++p->position;
  finish_value(p);
  return 1;
}

static int literal(RinzuJsonParser *p, const unsigned char *bytes, const char *word,
                   uint32_t length, unsigned kind, RinzuJsonBatch *batch) {
  uint32_t start = p->position;
  for (uint32_t i = 0; i < length; ++i) {
    if (p->position == p->length || bytes[p->position] != (unsigned char)word[i]) {
      unexpected(p, bytes, RZ_JSON_TOKEN);
      return 0;
    }
    ++p->position;
  }
  p->token = (Token){.start = start, .end = p->position};
  return scalar(p, bytes, kind, batch);
}

int rinzu_json_step(RinzuJsonParser *p, const unsigned char *bytes, RinzuJsonBatch *batch) {
  if ((batch != NULL) != (p->schema.types != NULL))
    rinzu_native_fatal(86);
  if (batch) {
    batch->count = 0;
    if (p->lex != LEX_STRING || p->token.offset == RZ_JSON_NONE || p->key)
      p->decoded.length = 0;
  }
  uint32_t end = p->length - p->position > SCAN_BYTES ? p->position + SCAN_BYTES : p->length;
  for (uint32_t work = 0; work < SCAN_WORK && !p->error.code && !p->finished &&
                          (!batch || batch->count < RZ_JSON_BATCH_SIZE);
       ++work) {
    if (p->lex == LEX_STRING) {
      if (!scan_string(p, bytes, end))
        break;
      if (p->key)
        finish_key(p);
      else if (!scalar(p, bytes, NODE_TEXT, batch))
        break;
      continue;
    }
    if (p->lex == LEX_NUMBER) {
      if (!scan_number(p, bytes, end) || !scalar(p, bytes, NODE_NUMBER, batch))
        break;
      continue;
    }
    if (p->position >= end && p->position != p->length)
      break;
    p->position = skip_space(bytes, p->position, end);
    if (p->position == p->length) {
      if (p->root_done)
        p->finished = 1;
      else
        decode_error(p, RZ_JSON_END, p->length);
      break;
    }
    if (p->position == end)
      break;
    if (p->root_done) {
      unexpected(p, bytes, RZ_JSON_TRAILING);
      break;
    }
    unsigned c = bytes[p->position];
    if (p->stack.length) {
      Frame *frame = top(p);
      unsigned closer = frame->kind == NODE_OBJECT ? '}' : ']';
      if (frame->next == COLON) {
        if (!key_colon(p, bytes, batch))
          break;
        continue;
      }
      if (c == closer && (frame->next == FIRST || frame->next == COMMA)) {
        if (!close_container(p, bytes, batch, &work))
          break;
        continue;
      }
      if (frame->next == COMMA) {
        if (c != ',') {
          unexpected(p, bytes, RZ_JSON_TOKEN);
          break;
        }
        ++p->position;
        frame->next = NEXT;
        continue;
      }
      if (frame->kind == NODE_OBJECT && frame->next != VALUE) {
        if (c != '"') {
          unexpected(p, bytes, RZ_JSON_TOKEN);
          break;
        }
        if (!check_node(p, 0))
          break;
        begin_string(p, 1);
        if (!scan_string(p, bytes, end))
          break;
        finish_key(p);
        if (p->position < end && bytes[p->position] == ':' && !key_colon(p, bytes, batch))
          break;
        continue;
      }
    }
    if (c != '[' && c != '{' && c != '"' && c != 'n' && c != 't' && c != 'f' && c != '-' &&
        !digit(c)) {
      unexpected(p, bytes, RZ_JSON_TOKEN);
      break;
    }
    if (!check_node(p, c == '[' || c == '{'))
      break;
    switch (c) {
    case '[':
    case '{':
      (void)open_container(p, bytes, c == '[' ? NODE_ARRAY : NODE_OBJECT, batch);
      break;
    case '"':
      begin_string(p, 0);
      if (!scan_string(p, bytes, end))
        goto step_done;
      (void)scalar(p, bytes, NODE_TEXT, batch);
      break;
    case 'n':
      (void)literal(p, bytes, "null", 4, NODE_NULL, batch);
      break;
    case 't':
      (void)literal(p, bytes, "true", 4, NODE_TRUE, batch);
      break;
    case 'f':
      (void)literal(p, bytes, "false", 5, NODE_FALSE, batch);
      break;
    default:
      begin_number(p, bytes);
      if (!scan_number(p, bytes, end))
        goto step_done;
      (void)scalar(p, bytes, NODE_NUMBER, batch);
      break;
    }
    /* A literal or escape may consume a few bytes over the scan boundary. */
    if (p->position > end)
      break;
  }
step_done:
  if (batch)
    batch->decoded = (RinzuJsonSlice){p->decoded.data, p->decoded.length};
  return p->error.code ? RZ_JSON_FAILED : p->finished ? RZ_JSON_DONE : RZ_JSON_MORE;
}

RinzuJsonBuffer rinzu_json_index(RinzuJsonParser *p) {
  if (!p->finished || p->error.code || p->schema.types)
    rinzu_native_fatal(86);
  RinzuJsonBuffer *regions[] = {&p->nodes, &p->containers, &p->texts,
                                &p->edges, &p->hashes,     &p->decoded};
  uint32_t offsets[sizeof regions / sizeof *regions];
  size_t size = sizeof offsets;
  for (size_t i = 0; i < sizeof offsets / sizeof *offsets; ++i) {
    offsets[i] = (uint32_t)size;
    if (regions[i]->length > UINT32_MAX - size)
      rinzu_native_fatal(71);
    size += regions[i]->length;
  }
  RinzuJsonBuffer buffer = {.data = malloc(size + 1), .length = size, .capacity = size + 1};
  if (!buffer.data)
    rinzu_native_fatal(71);
  rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, size + 1);
  memcpy(buffer.data, offsets, sizeof offsets);
  for (size_t i = 0; i < sizeof offsets / sizeof *offsets; ++i) {
    if (regions[i]->length)
      memcpy(buffer.data + offsets[i], regions[i]->data, regions[i]->length);
    rinzu_json_buffer_free(regions[i]);
  }
  return buffer;
}

void rinzu_json_document(RinzuJsonDocument *doc, RinzuJsonSlice source, RinzuJsonSlice index) {
  uint32_t offsets[6];
  if (index.length < sizeof offsets + sizeof(Node))
    rinzu_native_fatal(86);
  memcpy(offsets, index.data, sizeof offsets);
  if (offsets[0] != sizeof offsets || offsets[5] > index.length)
    rinzu_native_fatal(86);
  for (size_t i = 1; i < sizeof offsets / sizeof *offsets; ++i)
    if (offsets[i - 1] > offsets[i])
      rinzu_native_fatal(86);
  *doc = (RinzuJsonDocument){index.data, source,     offsets[0], offsets[1],
                             offsets[2], offsets[3], offsets[4], offsets[5]};
}

static uint32_t read_word(const unsigned char *bytes, uint32_t index) {
  uint32_t value;
  memcpy(&value, bytes + (size_t)index * sizeof value, sizeof value);
  return value;
}

static Node read_node(const RinzuJsonDocument *doc, uint32_t node) {
  Node value;
  if (node >= (doc->containers - doc->nodes) / sizeof value)
    rinzu_native_fatal(86);
  memcpy(&value, doc->index + doc->nodes + (size_t)node * sizeof value, sizeof value);
  return value;
}

static Container read_container(const RinzuJsonDocument *doc, uint32_t container) {
  Container value;
  memcpy(&value, doc->index + doc->containers + (size_t)container * sizeof value, sizeof value);
  return value;
}

static Text read_text(const RinzuJsonDocument *doc, uint32_t text) {
  Text value;
  memcpy(&value, doc->index + doc->texts + (size_t)text * sizeof value, sizeof value);
  return value;
}

RinzuJsonNode rinzu_json_node(const RinzuJsonDocument *doc, uint32_t node) {
  Node value = read_node(doc, node);
  unsigned kind = value.tagged_start >> START_BITS;
  RinzuJsonNode output = {.kind = public_kind(kind),
                          .start = value.tagged_start & START_MASK,
                          .end = value.detail,
                          .nodes = 1};
  if (kind == NODE_ESCAPED) {
    output.end = read_text(doc, value.detail).end;
  } else if (kind == NODE_ARRAY || kind == NODE_OBJECT) {
    Container c = read_container(doc, value.detail);
    output.end = c.end;
    output.depth = c.depth;
    output.nodes = c.nodes;
    output.length = c.count;
  }
  return output;
}

static uint32_t child_at(const RinzuJsonDocument *doc, Container c, uint32_t index) {
  unsigned flags = c.first & EDGE_FLAGS;
  if (flags)
    return (c.first & ~EDGE_FLAGS) + index * (flags == CONTIGUOUS_ARRAY ? 1 : 2);
  return read_word(doc->index + doc->edges, c.first + index);
}

uint32_t rinzu_json_child(const RinzuJsonDocument *doc, uint32_t node, uint32_t index) {
  Node value = read_node(doc, node);
  Container c = read_container(doc, value.detail);
  return index < c.count ? child_at(doc, c, index) : RZ_JSON_NONE;
}

RinzuJsonSlice rinzu_json_text(const RinzuJsonDocument *doc, uint32_t node) {
  Node value = read_node(doc, node);
  unsigned kind = value.tagged_start >> START_BITS;
  uint32_t start = value.tagged_start & START_MASK;
  if (kind == NODE_ESCAPED) {
    Text text = read_text(doc, value.detail);
    return (RinzuJsonSlice){doc->index + doc->decoded + text.offset, text.length};
  }
  if (kind == NODE_TEXT)
    return (RinzuJsonSlice){doc->source.data + start + 1, value.detail - start - 2};
  return (RinzuJsonSlice){doc->source.data + start, value.detail - start};
}

uint32_t rinzu_json_find(const RinzuJsonDocument *doc, uint32_t node, RinzuJsonSlice key) {
  Node value = read_node(doc, node);
  Container c = read_container(doc, value.detail);
  if (c.count <= SMALL_OBJECT) {
    for (uint32_t i = 0; i < c.count; ++i) {
      uint32_t child = child_at(doc, c, i);
      if (!compare_key(key, rinzu_json_text(doc, child)))
        return child + 1;
    }
  } else if (c.lookup & INDEX_SORTED) {
    const unsigned char *order =
        doc->index + doc->hashes + (size_t)(c.lookup & ~INDEX_SORTED) * sizeof(uint32_t);
    uint32_t lo = 0, hi = c.count;
    while (lo < hi) {
      uint32_t middle = lo + (hi - lo) / 2;
      uint32_t child = child_at(doc, c, read_word(order, middle));
      int cmp = compare_key(key, rinzu_json_text(doc, child));
      if (!cmp)
        return child + 1;
      if (cmp < 0)
        hi = middle;
      else
        lo = middle + 1;
    }
  } else {
    const unsigned char *table = doc->index + doc->hashes + (size_t)c.lookup * sizeof(uint32_t);
    uint32_t mask = hash_capacity(c.count) - 1;
    uint32_t slot = (uint32_t)hash_key(key) & mask;
    for (unsigned i = 0; i < MAX_PROBES; ++i, slot = (slot + 1) & mask) {
      uint32_t candidate = read_word(table, slot);
      if (candidate == RZ_JSON_NONE)
        break;
      uint32_t child = child_at(doc, c, candidate);
      if (!compare_key(key, rinzu_json_text(doc, child)))
        return child + 1;
    }
  }
  return RZ_JSON_NONE;
}

RinzuJsonSlice rinzu_json_event_text(const RinzuJsonEvent *event, RinzuJsonSlice source,
                                     RinzuJsonSlice decoded) {
  RinzuJsonSlice storage = event->decoded ? decoded : source;
  return (RinzuJsonSlice){storage.data + event->start, event->length};
}

int rinzu_json_append(RinzuJsonBuffer *buffer, size_t limit, RinzuJsonSlice text) {
  if (text.length > limit - buffer->length)
    return 0;
  append(buffer, text.data, text.length);
  return 1;
}

int rinzu_json_escape(RinzuJsonBuffer *buffer, size_t limit, RinzuJsonSlice text) {
  size_t pos = 0, start = 0;
  while (pos < text.length) {
    unsigned c = text.data[pos];
    if (c < 0x20 || c == '"' || c == '\\') {
      if (!rinzu_json_append(buffer, limit, (RinzuJsonSlice){text.data + start, pos - start}))
        return 0;
      unsigned char escape[6] = {'\\', (unsigned char)c, '0', '0', '0', '0'};
      size_t n = 2;
      switch (c) {
      case '\b':
        escape[1] = 'b';
        break;
      case '\f':
        escape[1] = 'f';
        break;
      case '\n':
        escape[1] = 'n';
        break;
      case '\r':
        escape[1] = 'r';
        break;
      case '\t':
        escape[1] = 't';
        break;
      case '"':
      case '\\':
        break;
      default: {
        const unsigned char hex[] = "0123456789abcdef";
        escape[1] = 'u';
        escape[4] = hex[c >> 4];
        escape[5] = hex[c & 15];
        n = 6;
      }
      }
      if (!rinzu_json_append(buffer, limit, (RinzuJsonSlice){escape, n}))
        return 0;
      start = pos + 1;
    }
    ++pos;
  }
  return pos == start ||
         rinzu_json_append(buffer, limit, (RinzuJsonSlice){text.data + start, pos - start});
}
