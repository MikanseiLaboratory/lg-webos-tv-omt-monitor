/*
 * SDL2 presenter for the OMT monitor.
 *
 * SDL handles the webOS shell role and remote-control input, which a raw
 * Wayland client does not get. Rendering uses a streaming BGRA texture.
 */

#include <SDL2/SDL.h>
#include <GLES3/gl3.h>
#include <stdio.h>
#include <string.h>

static SDL_Window *g_win = NULL;
static SDL_Renderer *g_ren = NULL;
static SDL_Texture *g_tex = NULL;
static int g_tex_w = 0, g_tex_h = 0;
static int g_w = 0, g_h = 0;
static void (*g_log)(const char *) = NULL;
static int g_use_gl = 0;
static SDL_GLContext g_glctx = NULL;
static GLuint g_gl_prog = 0, g_gl_tex = 0, g_gl_vbo = 0, g_gl_pbo[2] = {0, 0};
static int g_gl_pbo_idx = 0;
static GLint g_a_pos = -1, g_a_uv = -1, g_u_tex = -1;
void sdl_presenter_set_log(void (*fn)(const char *)) { g_log = fn; }
static void plog(const char *m) { if (g_log) g_log(m); }

int sdl_presenter_init(int w, int h, int want_gl)
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
  Uint32 win_flags = SDL_WINDOW_FULLSCREEN_DESKTOP;
  if (want_gl) {
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_ES);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 3);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 0);
    win_flags |= SDL_WINDOW_OPENGL;
  }
  g_win = SDL_CreateWindow("OMT Monitor", SDL_WINDOWPOS_CENTERED,
                           SDL_WINDOWPOS_CENTERED, dw, dh, win_flags);
  if (!g_win) {
    fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError());
    return -2;
  }
  if (want_gl) {
    g_glctx = SDL_GL_CreateContext(g_win);
    if (!g_glctx) {
      fprintf(stderr, "SDL_GL_CreateContext: %s\n", SDL_GetError());
      return -5;
    }
    SDL_GL_MakeCurrent(g_win, g_glctx);
    g_use_gl = 1;
    { char m[128]; snprintf(m, sizeof(m), "GL %s", (const char *)glGetString(GL_VERSION)); plog(m); }
    /* shaders (GLSL ES 1.00 works inside an ES3 context) */
    static const char *vs =
        "attribute vec2 a_pos;\nattribute vec2 a_uv;\nvarying vec2 v_uv;\n"
        "void main(){ v_uv=a_uv; gl_Position=vec4(a_pos,0.0,1.0); }\n";
    static const char *fs =
        "precision mediump float;\nvarying vec2 v_uv;\nuniform sampler2D u_tex;\n"
        "void main(){ gl_FragColor=texture2D(u_tex,v_uv); }\n";
    GLuint v = glCreateShader(GL_VERTEX_SHADER);
    glShaderSource(v, 1, &vs, NULL); glCompileShader(v);
    GLuint f = glCreateShader(GL_FRAGMENT_SHADER);
    glShaderSource(f, 1, &fs, NULL); glCompileShader(f);
    g_gl_prog = glCreateProgram();
    glAttachShader(g_gl_prog, v); glAttachShader(g_gl_prog, f); glLinkProgram(g_gl_prog);
    g_a_pos = glGetAttribLocation(g_gl_prog, "a_pos");
    g_a_uv = glGetAttribLocation(g_gl_prog, "a_uv");
    g_u_tex = glGetUniformLocation(g_gl_prog, "u_tex");
    static const float verts[] = {-1,-1,0,1, 1,-1,1,1, -1,1,0,0, 1,1,1,0};
    glGenBuffers(1, &g_gl_vbo);
    glBindBuffer(GL_ARRAY_BUFFER, g_gl_vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(verts), verts, GL_STATIC_DRAW);
    glGenTextures(1, &g_gl_tex);
    glBindTexture(GL_TEXTURE_2D, g_gl_tex);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glGenBuffers(2, g_gl_pbo);
    return 0;
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
  if (!g_ren && !g_use_gl)
    return -1;

  if (!g_use_gl && (!g_tex || w != g_tex_w || h != g_tex_h)) {
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
  if (g_use_gl) {
    size_t bytes = (size_t)w * (size_t)h * 4;
    glBindBuffer(GL_PIXEL_UNPACK_BUFFER, g_gl_pbo[g_gl_pbo_idx]);
    glBufferData(GL_PIXEL_UNPACK_BUFFER, (GLsizeiptr)bytes, NULL, GL_STREAM_DRAW);
    void *ptr = glMapBufferRange(GL_PIXEL_UNPACK_BUFFER, 0, (GLsizeiptr)bytes,
                                 GL_MAP_WRITE_BIT | GL_MAP_INVALIDATE_BUFFER_BIT);
    if (ptr) {
      memcpy(ptr, bgra, bytes);
      glUnmapBuffer(GL_PIXEL_UNPACK_BUFFER);
    }
    glBindTexture(GL_TEXTURE_2D, g_gl_tex);
    if (w != g_tex_w || h != g_tex_h) {
      glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
      g_tex_w = w;
      g_tex_h = h;
    }
    glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, 0);
    glBindBuffer(GL_PIXEL_UNPACK_BUFFER, 0);
    g_gl_pbo_idx ^= 1;
  } else {
    if (SDL_UpdateTexture(g_tex, NULL, bgra, w * 4) != 0)
      return -3;
  }
  Uint32 t1 = SDL_GetTicks();

  if (g_use_gl) {
    glViewport(0, 0, g_w, g_h);
    glClearColor(0, 0, 0, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(g_gl_prog);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, g_gl_tex);
    glUniform1i(g_u_tex, 0);
    glBindBuffer(GL_ARRAY_BUFFER, g_gl_vbo);
    glEnableVertexAttribArray((GLuint)g_a_pos);
    glEnableVertexAttribArray((GLuint)g_a_uv);
    glVertexAttribPointer((GLuint)g_a_pos, 2, GL_FLOAT, GL_FALSE, 16, (void *)0);
    glVertexAttribPointer((GLuint)g_a_uv, 2, GL_FLOAT, GL_FALSE, 16, (void *)8);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    SDL_GL_SwapWindow(g_win);
  } else {
    SDL_RenderClear(g_ren);
    SDL_RenderCopy(g_ren, g_tex, NULL, NULL);
    SDL_RenderPresent(g_ren);
  }
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
