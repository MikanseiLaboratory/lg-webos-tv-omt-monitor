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
static void (*g_log)(const char *) = NULL;
void sdl_presenter_set_log(void (*fn)(const char *)) { g_log = fn; }
static void plog(const char *m) { if (g_log) g_log(m); }

int sdl_presenter_init(int w, int h)
{
  if (SDL_Init(SDL_INIT_VIDEO) != 0) {
    fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
    return -1;
  }
  { char m[96]; snprintf(m, sizeof(m), "SDL driver %s", SDL_GetCurrentVideoDriver()); plog(m); }

  SDL_DisplayMode dm;
  int dw = w, dh = h;
  if (SDL_GetDesktopDisplayMode(0, &dm) == 0) {
    { char m[96]; snprintf(m, sizeof(m), "desktop mode %dx%d", dm.w, dm.h); plog(m); }
    dw = dm.w;
    dh = dm.h;
  }
  g_w = dw;
  g_h = dh;
  g_win = SDL_CreateWindow("OMT Monitor", SDL_WINDOWPOS_CENTERED,
                           SDL_WINDOWPOS_CENTERED, dw, dh,
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
  {
    int ww = 0, wh = 0, ow = 0, oh = 0;
    SDL_GetWindowSize(g_win, &ww, &wh);
    SDL_GetRendererOutputSize(g_ren, &ow, &oh);
    { char m[128]; snprintf(m, sizeof(m), "window %dx%d output %dx%d", ww, wh, ow, oh); plog(m); }
  }
  return 0;
}

void sdl_presenter_size(int *w, int *h)
{
  int ow = 0, oh = 0;
  if (g_ren && SDL_GetRendererOutputSize(g_ren, &ow, &oh) == 0 && ow > 0 && oh > 0) {
    *w = ow;
    *h = oh;
  } else {
    *w = g_w;
    *h = g_h;
  }
}

/* Rolling averages of the two upload steps (milliseconds). */
static double g_upload_ms = 0.0, g_present_ms = 0.0;

void sdl_presenter_timings(double *upload_ms, double *present_ms)
{
  *upload_ms = g_upload_ms;
  *present_ms = g_present_ms;
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
    /* Map texture pixels onto the whole output so a mismatched surface size
       (compositor vs renderer) cannot crop the frame. */
    SDL_RenderSetLogicalSize(g_ren, w, h);
  }

  Uint32 t0 = SDL_GetTicks();
  if (SDL_UpdateTexture(g_tex, NULL, bgra, w * 4) != 0)
    return -3;
  Uint32 t1 = SDL_GetTicks();

  SDL_RenderClear(g_ren);
  SDL_RenderCopy(g_ren, g_tex, NULL, NULL);
  SDL_RenderPresent(g_ren);
  Uint32 t2 = SDL_GetTicks();

  g_upload_ms = g_upload_ms * 0.9 + (double)(t1 - t0) * 0.1;
  g_present_ms = g_present_ms * 0.9 + (double)(t2 - t1) * 0.1;
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
