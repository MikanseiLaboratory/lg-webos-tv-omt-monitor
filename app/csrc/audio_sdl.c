/*
 * SDL2 audio output for OMT FPA1 (planar float32) audio.
 *
 * The receiver pushes interleaved float frames; the SDL callback drains a
 * fixed-size ring buffer. When the ring runs dry the callback emits silence.
 */

#include <SDL2/SDL.h>
#include <string.h>

static SDL_AudioDeviceID g_dev = 0;
static SDL_mutex *g_lock = NULL;
static float *g_ring = NULL;
static int g_ring_samples = 0; /* capacity in floats */
static int g_read = 0;
static int g_write = 0;
static int g_count = 0;        /* floats currently buffered */
static int g_channels = 2;

static void audio_callback(void *userdata, Uint8 *stream, int len)
{
  (void)userdata;
  float *out = (float *)stream;
  int need = len / (int)sizeof(float);

  SDL_LockMutex(g_lock);
  int n = need < g_count ? need : g_count;
  for (int i = 0; i < n; i++) {
    out[i] = g_ring[g_read];
    g_read = (g_read + 1) % g_ring_samples;
  }
  g_count -= n;
  SDL_UnlockMutex(g_lock);

  if (n < need)
    memset(out + n, 0, (size_t)(need - n) * sizeof(float));
}

int audio_open(int sample_rate, int channels)
{
  if (g_dev)
    return 0;
  if (channels < 1 || channels > 8)
    return -1;

  if (SDL_InitSubSystem(SDL_INIT_AUDIO) != 0)
    return -2;

  SDL_AudioSpec want, have;
  memset(&want, 0, sizeof(want));
  want.freq = sample_rate;
  want.format = AUDIO_F32SYS;
  want.channels = (Uint8)channels;
  want.samples = 1024;
  want.callback = audio_callback;

  g_channels = channels;
  g_dev = SDL_OpenAudioDevice(NULL, 0, &want, &have, 0);
  if (!g_dev)
    return -3;

  g_lock = SDL_CreateMutex();
  g_ring_samples = have.freq * channels * 2; /* 2 seconds */
  g_ring = (float *)SDL_malloc((size_t)g_ring_samples * sizeof(float));
  if (!g_ring || !g_lock)
    return -4;

  g_read = g_write = g_count = 0;
  SDL_PauseAudioDevice(g_dev, 0); /* start playback */
  return 0;
}

void audio_push(const float *interleaved, int frames)
{
  if (!g_dev || !g_ring)
    return;
  int samples = frames * g_channels;

  SDL_LockMutex(g_lock);
  int space = g_ring_samples - g_count;
  int n = samples < space ? samples : space;
  for (int i = 0; i < n; i++) {
    g_ring[g_write] = interleaved[i];
    g_write = (g_write + 1) % g_ring_samples;
  }
  g_count += n;
  SDL_UnlockMutex(g_lock);
}

int audio_level_frames(void)
{
  if (!g_dev || !g_ring || g_channels <= 0)
    return 0;
  return g_count / g_channels;
}

void audio_close(void)
{
  if (g_dev) {
    SDL_CloseAudioDevice(g_dev);
    g_dev = 0;
  }
  if (g_ring) {
    SDL_free(g_ring);
    g_ring = NULL;
  }
  if (g_lock) {
    SDL_DestroyMutex(g_lock);
    g_lock = NULL;
  }
}
