#ifndef RINZU_CONSUMPTION_H
#define RINZU_CONSUMPTION_H
#include <stdint.h>
uint64_t rinzu_consume_string(const char *(*producer)(uint64_t), uint64_t count);
uint64_t rinzu_consume_bytes4(const unsigned char *(*producer)(uint64_t), uint64_t count);
#endif
