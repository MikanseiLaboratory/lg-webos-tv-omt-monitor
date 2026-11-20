/* Minimal getauxval() for webOS' ancient 32-bit glibc sysroot. */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

unsigned long getauxval(unsigned long type) {
  static unsigned long cache_atomic[64];
  static unsigned long cache_value[64];
  static int cached[64];
  static int have_cache;
  (void)cache_atomic;
  (void)cache_value;

  if (!have_cache) {
    FILE *f = fopen("/proc/self/auxv", "rb");
    if (f) {
      unsigned long pair[2];
      while (fread(pair, sizeof(pair), 1, f) == 1) {
        if (pair[0] < 64 && !cached[pair[0]]) {
          cache_atomic[pair[0]] = pair[0];
          cache_value[pair[0]] = pair[1];
          cached[pair[0]] = 1;
        }
      }
      fclose(f);
    }
    have_cache = 1;
  }
  if (type < 64 && cached[type]) return cache_value[type];
  return 0;
}
