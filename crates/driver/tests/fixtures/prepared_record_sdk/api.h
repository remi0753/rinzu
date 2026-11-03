#ifndef PREPARED_RECORD_FIXTURE_H
#define PREPARED_RECORD_FIXTURE_H
#include <stdint.h>
struct Wide { int64_t left; uint64_t right; };
struct Single { int64_t value; };
struct Mixed { _Bool flag; int64_t count; };
const struct Wide *prepared_wide(int64_t seed, int64_t (*callback)(int64_t));
const struct Single *prepared_single(int64_t value);
const struct Mixed *prepared_mixed(int64_t kind);
void prepared_mutate(void);
#endif
