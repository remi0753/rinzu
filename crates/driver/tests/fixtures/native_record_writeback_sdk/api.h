#ifndef NATIVE_RECORD_WRITEBACK_H
#define NATIVE_RECORD_WRITEBACK_H
#include <stdint.h>
#include <stdbool.h>
struct Packet { int64_t x; bool valid; };
int64_t record_change(const struct Packet *value, int64_t amount);
bool record_aliases(const struct Packet *first, const struct Packet *second);
const struct Packet *record_echo(const struct Packet *value);
const struct Packet *record_fresh(void);
int64_t record_invoke(const struct Packet *value, int64_t (*handler)(void));
int64_t record_visit(const struct Packet *value, int64_t (*handler)(const struct Packet *));
void *record_probe(const struct Packet *value, void *context, int64_t (*handler)(void));
int64_t record_probe_entries(void);
int64_t record_run(const struct Packet *value, int64_t (*handler)(void));
int64_t record_run_entries(void);
void record_run_reset(void);
#endif
