#include "api.h"
int64_t record_change(const struct Packet *value, int64_t amount) {
  struct Packet *target = (struct Packet *)value;
  target->x += amount;
  target->valid = true;
  return -7; /* A business status still requires writeback. */
}
bool record_aliases(const struct Packet *first, const struct Packet *second) {
  if (first != second)
    return false;
  ((struct Packet *)first)->x += 2;
  return true;
}
const struct Packet *record_echo(const struct Packet *value) {
  return value;
}
const struct Packet *record_fresh(void) {
  static const struct Packet fresh = {9, true};
  return &fresh;
}
int64_t record_invoke(const struct Packet *value, int64_t (*handler)(void)) {
  ((struct Packet *)value)->x += 10;
  int64_t result = handler();
  ((struct Packet *)value)->x += 1;
  return result;
}
int64_t record_visit(const struct Packet *value, int64_t (*handler)(const struct Packet *)) {
  ((struct Packet *)value)->x += 10;
  int64_t result = handler(value);
  ((struct Packet *)value)->x += 1;
  return result;
}
static _Thread_local int64_t probes;
void *record_probe(const struct Packet *value, void *context, int64_t (*handler)(void)) {
  (void)value;
  (void)context;
  (void)handler;
  ++probes;
  return &probes;
}
int64_t record_probe_entries(void) {
  return probes;
}
static _Thread_local int64_t runs;
int64_t record_run(const struct Packet *value, int64_t (*handler)(void)) {
  ++runs;
  ((struct Packet *)value)->x += 10;
  ((struct Packet *)value)->valid = true;
  int64_t result = handler();
  ((struct Packet *)value)->x += 1;
  return result;
}
int64_t record_run_entries(void) {
  return runs;
}
void record_run_reset(void) {
  runs = 0;
}
