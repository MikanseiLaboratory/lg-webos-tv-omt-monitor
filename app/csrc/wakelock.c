/*
 * Keeps the TV awake while the monitor is displaying.
 *
 * webOS suspends the picture (Screen Saver / Screen Off) on input inactivity,
 * and a native app that renders with EGL is not registered as "video playing".
 * We therefore hold a luna-service connection and periodically ask tvpowerd to
 * bring the screen back to Active.
 */

#include <luna-service2/lunaservice.h>
#include <glib.h>
#include <stdio.h>
#include <string.h>

static LSHandle *g_handle = NULL;
static GMainLoop *g_loop = NULL;

static gboolean wakelock_tick(gpointer data)
{
  (void)data;
  if (!g_handle)
    return TRUE;
  LSError lerr;
  LSErrorInit(&lerr);
  if (!LSCallOneReply(g_handle,
                      "luna://com.webos.service.tvpower/power/turnOnScreen",
                      "{\"subscribe\":true}", NULL, NULL, NULL, &lerr)) {
    LSErrorFree(&lerr);
  }
  return TRUE;
}

static gboolean wakelock_register(gpointer data)
{
  (void)data;
  if (!g_handle)
    return FALSE;
  LSError lerr;
  LSErrorInit(&lerr);
  if (!LSCallOneReply(g_handle,
                      "luna://com.webos.service.tvpower/power/registerScreenSaverRequest",
                      "{\"clientName\":\"org.mikansei.omt-monitor\",\"subscribe\":true}",
                      NULL, NULL, NULL, &lerr)) {
    LSErrorFree(&lerr);
  }
  return FALSE;
}

int wakelock_start(void)
{
  LSError err;
  LSErrorInit(&err);

  if (!LSRegister("org.mikansei.omt-monitor", &g_handle, &err)) {
    fprintf(stderr, "wakelock: LSRegister failed: %s\n", err.message);
    LSErrorFree(&err);
    return -1;
  }

  g_loop = g_main_loop_new(NULL, FALSE);
  LSErrorInit(&err);
  if (!LSGmainAttach(g_handle, g_loop, &err)) {
    fprintf(stderr, "wakelock: LSGmainAttach failed: %s\n", err.message);
    LSErrorFree(&err);
    return -2;
  }

  wakelock_tick(NULL);          /* wake the screen immediately */
  wakelock_register(NULL);      /* ask to be consulted before the screensaver */
  g_timeout_add_seconds(30, wakelock_tick, NULL);

  g_main_loop_run(g_loop);      /* blocks this thread */
  return 0;
}
