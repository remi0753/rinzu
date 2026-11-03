#include <stdlib.h>
#include <stdint.h>
static size_t c06_allocations, c06_fail_at;
static void *c06_malloc(size_t size) {++c06_allocations;return c06_fail_at==c06_allocations?NULL:malloc(size);}
static void *c06_calloc(size_t count,size_t size) {++c06_allocations;return c06_fail_at==c06_allocations?NULL:calloc(count,size);}
#define malloc c06_malloc
#define calloc c06_calloc
