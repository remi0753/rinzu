#ifndef RINZU_SCALAR_H
#define RINZU_SCALAR_H
#include <stdint.h>
#include <math.h>
#include <string.h>

/* Private managed-value slots carry canonical bits, distinct from the physical native C ABI. */
enum rinzu_scalar_kind {
  RINZU_SCALAR_I8,
  RINZU_SCALAR_I16,
  RINZU_SCALAR_I32,
  RINZU_SCALAR_I64,
  RINZU_SCALAR_U8,
  RINZU_SCALAR_U16,
  RINZU_SCALAR_U32,
  RINZU_SCALAR_U64,
  RINZU_SCALAR_F32,
  RINZU_SCALAR_F64,
  RINZU_SCALAR_BOOL
};
typedef struct {
  uint64_t bits;
  int32_t status;
} rinzu_scalar_result_t;
static inline unsigned rinzu_scalar_width(unsigned kind) {
  switch (kind) {
  case 0:
  case 4:
  case 10:
    return 8;
  case 1:
  case 5:
    return 16;
  case 2:
  case 6:
  case 8:
    return 32;
  case 3:
  case 7:
  case 9:
    return 64;
  default:
    return 0;
  }
}
static inline int rinzu_scalar_signed(unsigned kind) {
  return kind < 4;
}
static inline float rinzu_scalar_f32(uint64_t bits) {
  uint32_t low = (uint32_t)bits;
  float value;
  memcpy(&value, &low, sizeof(value));
  return value;
}
static inline double rinzu_scalar_f64(uint64_t bits) {
  double value;
  memcpy(&value, &bits, sizeof(value));
  return value;
}
static inline uint64_t rinzu_scalar_f32_bits(float value) {
  uint32_t bits;
  memcpy(&bits, &value, sizeof(bits));
  return bits;
}
static inline uint64_t rinzu_scalar_f64_bits(double value) {
  uint64_t bits;
  memcpy(&bits, &value, sizeof(bits));
  return bits;
}
static inline rinzu_scalar_result_t rinzu_scalar_arithmetic(unsigned kind, unsigned operation,
                                                            uint64_t left, uint64_t right) {
  rinzu_scalar_result_t result = {0, 0};
  /* Every 64-bit pattern is canonical for I64/U64. Use native-width checked
     arithmetic rather than reconstructing the generic 128-bit range. */
  if (kind == RINZU_SCALAR_I64) {
    int64_t a = (int64_t)left, b = (int64_t)right, value = 0;
    int overflow = 0;
    switch (operation) {
    case 0:
      overflow = __builtin_sub_overflow((int64_t)0, a, &value);
      break;
    case 1:
      overflow = __builtin_add_overflow(a, b, &value);
      break;
    case 2:
      overflow = __builtin_sub_overflow(a, b, &value);
      break;
    case 3:
      overflow = __builtin_mul_overflow(a, b, &value);
      break;
    case 4:
    case 5:
      if (!b || (a == INT64_MIN && b == -1)) {
        result.status = 1;
        return result;
      }
      value = operation == 4 ? a / b : a % b;
      break;
    default:
      result.status = 2;
      return result;
    }
    if (overflow)
      result.status = 1;
    else
      result.bits = (uint64_t)value;
    return result;
  }
  if (kind == RINZU_SCALAR_U64) {
    uint64_t value = 0;
    int overflow = 0;
    switch (operation) {
    case 1:
      overflow = __builtin_add_overflow(left, right, &value);
      break;
    case 2:
      overflow = __builtin_sub_overflow(left, right, &value);
      break;
    case 3:
      overflow = __builtin_mul_overflow(left, right, &value);
      break;
    case 4:
    case 5:
      if (!right) {
        result.status = 1;
        return result;
      }
      value = operation == 4 ? left / right : left % right;
      break;
    default:
      result.status = 2;
      return result;
    }
    if (overflow)
      result.status = 1;
    else
      result.bits = value;
    return result;
  }
  if (kind < 8) {
    unsigned width = rinzu_scalar_width(kind);
    if (!width || width > 64) {
      result.status = 2;
      return result;
    }
    int is_signed = rinzu_scalar_signed(kind);
    __int128 minimum = is_signed ? -((__int128)1 << (width - 1)) : 0;
    __int128 maximum = is_signed ? ((__int128)1 << (width - 1)) - 1 : ((__int128)1 << width) - 1;
    __int128 a = is_signed ? (__int128)(int64_t)left : (__int128)left;
    __int128 b = is_signed ? (__int128)(int64_t)right : (__int128)right, value = 0;
    if (a < minimum || a > maximum || b < minimum || b > maximum) {
      result.status = 2;
      return result;
    }
    if (operation == 0 && !is_signed) {
      result.status = 2;
      return result;
    }
    if ((operation == 4 || operation == 5) && (b == 0 || (is_signed && a == minimum && b == -1))) {
      result.status = 1;
      return result;
    }
    switch (operation) {
    case 0:
      value = -a;
      break;
    case 1:
      value = a + b;
      break;
    case 2:
      value = a - b;
      break;
    case 3: {
      /* Validated operands fit in 64 bits; narrower result ranges are checked below. */
      int overflow;
      if (is_signed) {
        int64_t product;
        overflow = __builtin_mul_overflow((int64_t)a, (int64_t)b, &product);
        value = product;
      } else {
        uint64_t product;
        overflow = __builtin_mul_overflow((uint64_t)a, (uint64_t)b, &product);
        value = product;
      }
      if (overflow) {
        result.status = 1;
        return result;
      }
      break;
    }
    case 4:
      value =
          is_signed ? (__int128)((int64_t)a / (int64_t)b) : (__int128)((uint64_t)a / (uint64_t)b);
      break;
    case 5:
      value =
          is_signed ? (__int128)((int64_t)a % (int64_t)b) : (__int128)((uint64_t)a % (uint64_t)b);
      break;
    default:
      result.status = 2;
      return result;
    }
    if (value < minimum || value > maximum) {
      result.status = 1;
      return result;
    }
    result.bits = (uint64_t)value;
    return result;
  }
  if (kind == RINZU_SCALAR_F32) {
    float a = rinzu_scalar_f32(left), b = rinzu_scalar_f32(right), value;
    switch (operation) {
    case 0:
      value = -a;
      break;
    case 1:
      value = a + b;
      break;
    case 2:
      value = a - b;
      break;
    case 3:
      value = a * b;
      break;
    case 4:
      value = a / b;
      break;
    default:
      result.status = 2;
      return result;
    }
    result.bits = rinzu_scalar_f32_bits(value);
    return result;
  }
  if (kind == RINZU_SCALAR_F64) {
    double a = rinzu_scalar_f64(left), b = rinzu_scalar_f64(right), value;
    switch (operation) {
    case 0:
      value = -a;
      break;
    case 1:
      value = a + b;
      break;
    case 2:
      value = a - b;
      break;
    case 3:
      value = a * b;
      break;
    case 4:
      value = a / b;
      break;
    default:
      result.status = 2;
      return result;
    }
    result.bits = rinzu_scalar_f64_bits(value);
    return result;
  }
  result.status = 2;
  return result;
}
/* Conversion status 3 is a recoverable Error, distinct from arithmetic traps. */
static inline rinzu_scalar_result_t rinzu_scalar_convert(unsigned source, unsigned target,
                                                         uint64_t bits) {
  rinzu_scalar_result_t result = {0, 3};
  if (source >= RINZU_SCALAR_BOOL || target >= RINZU_SCALAR_BOOL)
    return result;
  if (source == target) {
    result.bits = bits;
    result.status = 0;
    return result;
  }
  int integer_source = source < 8;
  __int128 integer =
      integer_source ? (rinzu_scalar_signed(source) ? (__int128)(int64_t)bits : (__int128)bits) : 0;
  double floating = integer_source ? 0
                                   : (source == RINZU_SCALAR_F32 ? (double)rinzu_scalar_f32(bits)
                                                                 : rinzu_scalar_f64(bits));
  if (target < 8) {
    unsigned width = rinzu_scalar_width(target);
    if (!width || width > 64)
      return result;
    int is_signed = rinzu_scalar_signed(target);
    __int128 minimum = is_signed ? -((__int128)1 << (width - 1)) : 0;
    __int128 upper = (__int128)1 << (is_signed ? width - 1 : width);
    if (!integer_source) {
      if (!isfinite(floating) || trunc(floating) != floating || floating < (double)minimum ||
          floating >= (double)upper)
        return result;
      integer = (__int128)floating;
    }
    if (integer < minimum || integer >= upper)
      return result;
    result.bits = (uint64_t)integer;
  } else if (target == RINZU_SCALAR_F32) {
    /* Cast directly from the integer, avoiding double rounding through binary64. */
    float converted = integer_source ? (float)integer : (float)floating;
    if (!integer_source && isfinite(floating) && isinf(converted))
      return result;
    result.bits = rinzu_scalar_f32_bits(converted);
  } else {
    result.bits = rinzu_scalar_f64_bits(integer_source ? (double)integer : floating);
  }
  result.status = 0;
  return result;
}
#define RINZU_SCALAR_COMPARE(a, b)                                                                 \
  do {                                                                                             \
    switch (operation) {                                                                           \
    case 0:                                                                                        \
      return (a) == (b);                                                                           \
    case 1:                                                                                        \
      return (a) != (b);                                                                           \
    case 2:                                                                                        \
      return (a) < (b);                                                                            \
    case 3:                                                                                        \
      return (a) <= (b);                                                                           \
    case 4:                                                                                        \
      return (a) > (b);                                                                            \
    case 5:                                                                                        \
      return (a) >= (b);                                                                           \
    default:                                                                                       \
      return -1;                                                                                   \
    }                                                                                              \
  } while (0)
static inline int rinzu_scalar_compare(unsigned kind, unsigned operation, uint64_t left,
                                       uint64_t right) {
  if (kind < 4) {
    int64_t a = (int64_t)left, b = (int64_t)right;
    RINZU_SCALAR_COMPARE(a, b);
  }
  if (kind < 8) {
    RINZU_SCALAR_COMPARE(left, right);
  }
  if (kind == RINZU_SCALAR_F32) {
    float a = rinzu_scalar_f32(left), b = rinzu_scalar_f32(right);
    RINZU_SCALAR_COMPARE(a, b);
  }
  if (kind == RINZU_SCALAR_F64) {
    double a = rinzu_scalar_f64(left), b = rinzu_scalar_f64(right);
    RINZU_SCALAR_COMPARE(a, b);
  }
  if (kind == RINZU_SCALAR_BOOL && operation < 2 && left < 2 && right < 2) {
    RINZU_SCALAR_COMPARE(left, right);
  }
  return -1;
}
#undef RINZU_SCALAR_COMPARE
#endif
