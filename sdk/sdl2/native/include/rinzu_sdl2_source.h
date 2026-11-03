#ifndef RINZU_SDL2_SOURCE_H
#define RINZU_SDL2_SOURCE_H
#include <stdint.h>
struct Event {
  uint32_t kind, windowId;
  _Bool shouldQuit;
  int64_t sequence;
};
void *rinzu_sdl2_source_window(const char *, int32_t, int32_t);
_Bool rinzu_sdl2_source_poll(void *, struct Event *);
int32_t rinzu_sdl2_source_render(void *);
int32_t rinzu_sdl2_source_title(void *, const char *);
_Bool rinzu_sdl2_source_running(void *);
int32_t rinzu_sdl2_source_stop(void *);
int32_t rinzu_sdl2_source_destroy(void *);
int32_t rinzu_sdl2_source_destroy_owner_windows(void);
int64_t rinzu_sdl2_source_live_windows(void);
#endif
