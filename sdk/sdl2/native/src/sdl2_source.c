#include "rinzu_sdl2_source.h"
#include <SDL.h>
#include <pthread.h>
#include <stdlib.h>

/* SDL owns the native window/renderer. The non-owning Rinzu handle is valid
   during this SDK's run. Every SDL operation stays on the process Main thread. */
typedef struct {
  SDL_Window *window;
  SDL_Renderer *renderer;
  int running;
  int64_t sequence;
} Window;
static Window *active;
static int on_main(void) {
#ifdef __APPLE__
  return pthread_main_np();
#else
  return 1;
#endif
}
static Window *live(void *raw) {
  return on_main() && raw == active ? active : NULL;
}

void *rinzu_sdl2_source_window(const char *title, int32_t width, int32_t height) {
  if (!on_main() || active || !title || width <= 0 || height <= 0)
    return NULL;
  if (SDL_InitSubSystem(SDL_INIT_VIDEO))
    return NULL;
  SDL_Window *window = SDL_CreateWindow(title, SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED,
                                        width, height, SDL_WINDOW_SHOWN);
  if (!window) {
    SDL_QuitSubSystem(SDL_INIT_VIDEO);
    return NULL;
  }
  SDL_Renderer *renderer = SDL_CreateRenderer(window, -1, SDL_RENDERER_SOFTWARE);
  if (!renderer) {
    SDL_DestroyWindow(window);
    SDL_QuitSubSystem(SDL_INIT_VIDEO);
    return NULL;
  }
  Window *state = calloc(1, sizeof(*state));
  if (!state)
    _Exit(70);
  state->window = window;
  state->renderer = renderer;
  state->running = 1;
  active = state;
  return state;
}
_Bool rinzu_sdl2_source_poll(void *raw, struct Event *output) {
  Window *state = live(raw);
  if (!state || !output)
    return 0;
  SDL_Event event;
  if (!SDL_PollEvent(&event))
    return 0;
  uint32_t window = 0;
  int quit = event.type == SDL_QUIT;
  switch (event.type) {
  case SDL_WINDOWEVENT:
    window = event.window.windowID;
    quit = event.window.event == SDL_WINDOWEVENT_CLOSE;
    break;
  case SDL_KEYDOWN:
  case SDL_KEYUP:
    window = event.key.windowID;
    break;
  case SDL_MOUSEMOTION:
    window = event.motion.windowID;
    break;
  case SDL_MOUSEBUTTONDOWN:
  case SDL_MOUSEBUTTONUP:
    window = event.button.windowID;
    break;
  default:
    break;
  }
  if (state->sequence == INT64_MAX)
    _Exit(86);
  *output = (struct Event){event.type, window, !!quit, ++state->sequence};
  return 1;
}
int32_t rinzu_sdl2_source_render(void *raw) {
  Window *state = live(raw);
  if (!state)
    return 1;
  if (SDL_SetRenderDrawColor(state->renderer, 24, 36, 54, 255) || SDL_RenderClear(state->renderer))
    return 2;
  SDL_RenderPresent(state->renderer);
  return 0;
}
_Bool rinzu_sdl2_source_running(void *raw) {
  Window *state = live(raw);
  return state && state->running;
}
int32_t rinzu_sdl2_source_title(void *raw, const char *title) {
  Window *state = live(raw);
  if (!state || !title)
    return 1;
  SDL_SetWindowTitle(state->window, title);
  return 0;
}
int32_t rinzu_sdl2_source_stop(void *raw) {
  Window *state = live(raw);
  if (!state)
    return 1;
  state->running = 0;
  return 0;
}
int32_t rinzu_sdl2_source_destroy(void *raw) {
  Window *state = live(raw);
  if (!state)
    return 1;
  active = NULL;
  SDL_DestroyRenderer(state->renderer);
  SDL_DestroyWindow(state->window);
  SDL_QuitSubSystem(SDL_INIT_VIDEO);
  free(state);
  return 0;
}
int32_t rinzu_sdl2_source_destroy_owner_windows(void) {
  if (!on_main())
    return 1;
  return active ? rinzu_sdl2_source_destroy(active) : 0;
}
int64_t rinzu_sdl2_source_live_windows(void) {
  return on_main() ? (active != NULL) : -1;
}
