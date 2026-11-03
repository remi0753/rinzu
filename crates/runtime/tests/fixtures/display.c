#include "rinzu/display.h"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#define TEXT(s) {(const unsigned char *)(s), sizeof(s) - 1}
static const rinzu_display_field_t children[] = {{TEXT("children"), 1}};
static const rinzu_display_type_t types[] = {
    {.kind = RINZU_DISPLAY_RECORD, .name = TEXT("Node"), .fields = children, .count = 1},
    {.kind = RINZU_DISPLAY_LIST, .element = 0},
    {.kind = RINZU_DISPLAY_SCALAR, .scalar = 3},
    {.kind = RINZU_DISPLAY_MAP, .key = 2, .value = 1},
};
static int load(void *raw, size_t type, uint64_t bits, rinzu_display_node_t *node) {
  if (*(int *)raw)
    return 1;
  static rinzu_display_slot_t scratch[6];
  if (type == 3) {
    scratch[0] = (rinzu_display_slot_t){10, 0};
    scratch[1] = (rinzu_display_slot_t){0, 1};
    scratch[2] = (rinzu_display_slot_t){(uint64_t)-2, 0};
    scratch[3] = (rinzu_display_slot_t){0, 1};
    scratch[4] = (rinzu_display_slot_t){1, 0};
    scratch[5] = (rinzu_display_slot_t){0, 1};
    node->fields = scratch;
    node->count = 6;
    return 0;
  }
  if (type > 1 || bits > 20000)
    return 1;
  if (type == 1 && !bits) {
    node->count = 0;
    return 0;
  }
  scratch[0] = (rinzu_display_slot_t){bits - (type == 0), 1};
  node->fields = scratch;
  node->count = 1;
  return 0;
}
int main(void) {
  unsigned char *bytes = NULL;
  size_t length = 0;
  int failure = 0;
  assert(!rinzu_display_format(types, 4, 1, (rinzu_display_slot_t){20000, 1}, 0, load, &failure,
                               &bytes, &length));
  assert(length == 20000 * strlen("[Node(children: )]") + 2);
  assert(!memcmp(bytes, "[Node(children: [Node(children: ", 32));
  free(bytes);
  assert(!rinzu_display_format(types, 4, 3, (rinzu_display_slot_t){0, 1}, 0, load, &failure, &bytes,
                               &length));
  const char *map = "{-2: [], 1: [], 10: []}";
  assert(length == strlen(map) && !memcmp(bytes, map, length));
  free(bytes);
  failure = 1;
  assert(rinzu_display_format(types, 4, 3, (rinzu_display_slot_t){0, 1}, 0, load, &failure, &bytes,
                              &length));
  assert(!bytes && !length);
  assert(rinzu_display_format(types, 4, 2, (rinzu_display_slot_t){0, 1}, 0, load, &failure, &bytes,
                              &length));
  return 0;
}
