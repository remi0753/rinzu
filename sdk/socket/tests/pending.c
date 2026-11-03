/* Compare selected-slot enumeration to the original linear table order,
   including partial words, round-robin wrap and removal during a pass. */
#include "../native/src/socket.c"
#include <assert.h>
#include <stdio.h>

static size_t selections;
static void range(const uint64_t *bitmap, size_t start, size_t end) {
  for (;;) {
    size_t expected = start;
    while (expected < end && !((bitmap[expected / 64] >> (expected % 64)) & 1))
      ++expected;
    size_t actual = bitmap_next(bitmap, start, end);
    assert(actual == expected);
    ++selections;
    if (actual == end)
      return;
    start = actual + 1;
  }
}
static uint64_t random_word(uint64_t *state) {
  *state ^= *state << 13;
  *state ^= *state >> 7;
  *state ^= *state << 17;
  return *state;
}
static void layout(size_t count) {
  uint64_t bitmap[OP_WORDS] = {0};
  uint64_t state = 7;
  range(bitmap, 0, count);
  bitmap_add(bitmap, count - 1);
  range(bitmap, 0, count);
  range(bitmap, count - 1, count);
  range(bitmap, count, count);
  for (unsigned pattern = 0; pattern < 34; ++pattern) {
    for (size_t word = 0; word < OP_WORDS; ++word)
      bitmap[word] = pattern == 0 ? 0 : pattern == 1 ? UINT64_MAX : random_word(&state);
    for (size_t start = 0; start <= count; ++start) {
      range(bitmap, start, count);
      range(bitmap, 0, start);
    }
    /* End can lie inside a word even when higher bits in that word are set. */
    for (size_t end = 0; end <= count; ++end)
      range(bitmap, end > 65 ? end - 65 : 0, end);
  }
  memset(bitmap, 255, sizeof(bitmap));
  size_t start = 0;
  while (start < count) {
    size_t selected = bitmap_next(bitmap, start, count);
    range(bitmap, start, selected); /* The first selected slot excludes end. */
    if (selected == count)
      break;
    bitmap_remove(bitmap, selected);
    if (selected + 1 < count)
      bitmap_remove(bitmap, selected + 1);
    if (selected + 65 < count)
      bitmap_remove(bitmap, selected + 65);
    start = selected + 1;
    range(bitmap, start, count);
  }
  range(bitmap, count, count);
}
int main(void) {
  layout(OPS);
  layout(HANDLES);
  printf(
      "pending:operations=%d:handles=%d:empty:dense:sparse:wrap:partial-words:removal;checks=%zu\n",
      OPS, HANDLES, selections);
}
