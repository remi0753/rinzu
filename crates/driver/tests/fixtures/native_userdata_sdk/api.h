#ifndef USERDATA_FIXTURE_H
#define USERDATA_FIXTURE_H
#include <stdint.h>
int32_t userdata_probe(void *, void *, void *, void *, void *, uint64_t (*)(const uint8_t *),
                       int32_t);
uint64_t userdata_entries(void);
#endif
