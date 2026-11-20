/*
 * SDL2 presenter for the OMT monitor.
 *
 * SDL handles the webOS shell role and remote-control input, which a raw
 * Wayland client does not get. Rendering uses a streaming BGRA texture.
 */

#include <SDL2/SDL.h>
#include <stdio.h>
#include <string.h>

static SDL_Window *g_win = NULL;
static SDL_Renderer *g_ren = NULL;
static SDL_Texture *g_tex = NULL;
static int g_tex_w = 0, g_tex_h = 0;
static int g_w = 0, g_h = 0;

int sdl_presenter_init(int w, int h)
{
  if (SDL_Init(SDL_INIT_VIDEO) != 0) {
    fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
    return -1;
  }
  fprintf(stderr, "SDL video driver: %s\n", SDL_GetCurrentVideoDriver());

  g_w = w;
  g_h = h;
  g_win = SDL_CreateWindow("OMT Monitor", SDL_WINDOWPOS_CENTERED,
                           SDL_WINDOWPOS_CENTERED, w, h,
                           SDL_WINDOW_FULLSCREEN_DESKTOP);
  if (!g_win) {
    fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError());
    return -2;
  }
  g_ren = SDL_CreateRenderer(g_win, -1, SDL_RENDERER_ACCELERATED);
  if (!g_ren) {
    fprintf(stderr, "SDL_CreateRenderer: %s\n", SDL_GetError());
    return -3;
  }
  return 0;
}

int sdl_presenter_frame(const void *bgra, int w, int h)
{
  if (!g_ren)
    return -1;

  if (!g_tex || w != g_tex_w || h != g_tex_h) {
    if (g_tex)
      SDL_DestroyTexture(g_tex);
    g_tex = SDL_CreateTexture(g_ren, SDL_PIXELFORMAT_ARGB8888,
                              SDL_TEXTUREACCESS_STREAMING, w, h);
    if (!g_tex)
      return -2;
    g_tex_w = w;
    g_tex_h = h;
  }

  if (SDL_UpdateTexture(g_tex, NULL, bgra, w * 4) != 0)
    return -3;

  SDL_RenderClear(g_ren);
  SDL_RenderCopy(g_ren, g_tex, NULL, NULL);
  SDL_RenderPresent(g_ren);
  return 0;
}

/* Returns 1 and fills *code when a key press is pending. */
int sdl_presenter_poll_key(int *code)
{
  SDL_Event ev;
  while (SDL_PollEvent(&ev)) {
    if (ev.type == SDL_QUIT) {
      *code = 0;
      return 2;
    }
    if (ev.type == SDL_KEYDOWN) {
      *code = (int)ev.key.keysym.sym;
      return 1;
    }
  }
  return 0;
}

void sdl_presenter_shutdown(void)
{
  if (g_tex)
    SDL_DestroyTexture(g_tex);
  if (g_ren)
    SDL_DestroyRenderer(g_ren);
  if (g_win)
    SDL_DestroyWindow(g_win);
  SDL_Quit();
  g_tex = NULL;
  g_ren = NULL;
  g_win = NULL;
}
