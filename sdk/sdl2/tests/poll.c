#include "rinzu_sdl2_source.h"
#include <SDL.h>
#include <assert.h>
#include <pthread.h>

static void *foreign(void *raw) {
  (void)raw;
  assert(rinzu_sdl2_source_destroy_owner_windows() == 1);
  assert(rinzu_sdl2_source_live_windows() == -1);
  return NULL;
}

static int discard_event(void *context, SDL_Event *event) {
  (void)context;
  (void)event;
  return 0;
}

int main(void) {
  assert(!rinzu_sdl2_source_window("invalid", 0, 80));
  void *window = rinzu_sdl2_source_window("SDL source poll", 120, 80);
  assert(window);
  assert(rinzu_sdl2_source_live_windows() == 1);
  pthread_t owner;
  assert(pthread_create(&owner, NULL, foreign, NULL) == 0);
  assert(pthread_join(owner, NULL) == 0);
  assert(rinzu_sdl2_source_live_windows() == 1);
  assert(rinzu_sdl2_source_running(window));
  SDL_Event event;
  SDL_zero(event);
  event.type = SDL_KEYDOWN;
  event.key.windowID = 77;
  assert(SDL_PushEvent(&event) == 1);
  SDL_zero(event);
  event.type = SDL_QUIT;
  assert(SDL_PushEvent(&event) == 1);
  struct Event output = {0}, first = {0};
  int keyboard = 0, quit = 0;
  while (rinzu_sdl2_source_poll(window, &output)) {
    assert(output.sequence > 0);
    if (output.kind == SDL_KEYDOWN) {
      assert(output.windowId == 77 && !output.shouldQuit);
      first = output;
      keyboard++;
    }
    if (output.kind == SDL_QUIT) {
      assert(!output.windowId && output.shouldQuit && output.sequence > first.sequence);
      quit++;
    }
  }
  assert(keyboard == 1 && quit == 1 && first.kind == SDL_KEYDOWN && first.windowId == 77 &&
         !first.shouldQuit);
  /* OS window events may arrive after the drain above. Isolate only this
     empty-poll check from asynchronous input, then restore normal delivery. */
  SDL_SetEventFilter(discard_event, NULL);
  SDL_FlushEvents(SDL_FIRSTEVENT, SDL_LASTEVENT);
  struct Event unchanged = output;
  assert(!rinzu_sdl2_source_poll(window, &output));
  assert(output.kind == unchanged.kind && output.sequence == unchanged.sequence);
  SDL_SetEventFilter(NULL, NULL);
  assert(!rinzu_sdl2_source_render(window));
  assert(!rinzu_sdl2_source_stop(window));
  assert(!rinzu_sdl2_source_running(window));
  assert(!rinzu_sdl2_source_destroy(window));
  assert(!rinzu_sdl2_source_running(window));
  assert(rinzu_sdl2_source_destroy(window) == 1 && !SDL_WasInit(SDL_INIT_VIDEO));
  assert(rinzu_sdl2_source_destroy_owner_windows() == 0);
  assert(rinzu_sdl2_source_live_windows() == 0);
  (void)rinzu_sdl2_source_window("Discarded acquisition", 120, 80);
  assert(rinzu_sdl2_source_live_windows() == 1);
  assert(rinzu_sdl2_source_destroy_owner_windows() == 0);
  assert(rinzu_sdl2_source_live_windows() == 0 && !SDL_WasInit(SDL_INIT_VIDEO));
}
