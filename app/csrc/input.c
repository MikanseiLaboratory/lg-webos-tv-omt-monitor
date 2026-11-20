/*
 * webOS pointer-input socket client.
 *
 * Native apps receive remote input through the socket returned by
 * luna://com.webos.service.networkinput/getPointerInputSocket. Connecting raw is
 * not enough: the path must be obtained through luna first, which also registers
 * the caller as the input owner.
 *
 * The wire format is undocumented, so this module hexdumps what it receives and
 * the Rust side decodes key names from the printable part.
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <luna-service2/lunaservice.h>

#define FALLBACK_PATH "/tmp/netinput.pointer.sock"

static void (*g_hex)(const char *) = NULL;
static char g_path[256] = FALLBACK_PATH;
static volatile int g_got_path = 0;

LSHandle *wakelock_handle(void);

void input_set_logger(void (*fn)(const char *)) { g_hex = fn; }

static bool socket_reply(LSHandle *sh, LSMessage *msg, void *ctx)
{
  (void)sh;
  (void)ctx;
  const char *payload = LSMessageGetPayload(msg);
  if (!payload)
    return true;
  const char *p = strstr(payload, "\"socketPath\"");
  if (p) {
    p = strchr(p, ':');
    if (p) {
      p = strchr(p, '"');
      if (p) {
        p++;
        const char *end = strchr(p, '"');
        if (end) {
          size_t n = (size_t)(end - p);
          if (n < sizeof(g_path)) {
            memcpy(g_path, p, n);
            g_path[n] = 0;
            g_got_path = 1;
          }
        }
      }
    }
  }
  return true;
}

int input_probe(void)
{
  for (int i = 0; i < 50 && !wakelock_handle(); i++)
    usleep(100000);

  LSHandle *sh = wakelock_handle();
  if (sh) {
    LSError err;
    LSErrorInit(&err);
    LSCallOneReply(sh,
                   "luna://com.webos.service.networkinput/getPointerInputSocket",
                   "{}", socket_reply, NULL, NULL, &err);
    for (int i = 0; i < 30 && !g_got_path; i++)
      usleep(100000);
  }
  if (g_hex) {
    char m[300];
    snprintf(m, sizeof(m), "pointer socket: %s (luna=%d)", g_path, g_got_path);
    g_hex(m);
  }

  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0)
    return -1;

  struct sockaddr_un addr;
  memset(&addr, 0, sizeof(addr));
  addr.sun_family = AF_UNIX;
  strncpy(addr.sun_path, g_path, sizeof(addr.sun_path) - 1);

  if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
    close(fd);
    return -2;
  }

  if (g_hex)
    g_hex("pointer socket connected");

  char buf[256];
  char line[1024];
  for (;;) {
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    if (n <= 0)
      break;
    buf[n] = 0;
    int off = 0;
    for (ssize_t i = 0; i < n && off < (int)sizeof(line) - 4; i++)
      off += snprintf(line + off, sizeof(line) - off, "%02x ", (unsigned char)buf[i]);
    snprintf(line + off, sizeof(line) - off, "| %s", buf);
    if (g_hex)
      g_hex(line);
  }
  close(fd);
  return 0;
}
