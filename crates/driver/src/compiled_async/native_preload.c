#include <dlfcn.h>

int main(int argc, char **argv) {
  /*
   * The driver supplies only verified images with no load-time application
   * code. This process can populate the OS cache without changing the
   * application process's library namespace or initializer order.
   */
  for (int i = 1; i < argc; ++i)
    if (!dlopen(argv[i], RTLD_NOW | RTLD_LOCAL))
      return 1;
  return 0;
}
