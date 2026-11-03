#ifndef RINZU_FOREIGN_SDK_H
#define RINZU_FOREIGN_SDK_H
#include <stdint.h>
#include <stdbool.h>
typedef double (*ForeignScalar)(int8_t, int16_t, int32_t, int64_t, uint8_t, uint16_t, uint32_t,
                                uint64_t, float, double, bool, const char *, const uint8_t *);
typedef void (*ForeignUnit)(const char *, const uint8_t *);
typedef const char *(*ForeignText)(const char *);
typedef const uint8_t *(*ForeignBytes)(const uint8_t *);
void foreign_reset(int32_t);
int32_t foreign_finish(void);
void foreign_nested(void);
void foreign_body_marker(void);
void foreign_after_return(void);
int32_t foreign_ordinary(ForeignText);
int32_t foreign_run(ForeignScalar, ForeignUnit, ForeignText, ForeignBytes,
                    int32_t (*)(const char *), ForeignText, const uint8_t *(*)(const char *),
                    uint64_t (*)(const uint8_t *), int32_t (*)(const void *), int32_t);
const char *foreign_run_protected(ForeignScalar, ForeignUnit, ForeignText, ForeignBytes,
                                  int32_t (*)(const char *), ForeignText,
                                  const uint8_t *(*)(const char *), uint64_t (*)(const uint8_t *),
                                  int32_t (*)(const void *), int32_t);
#endif
