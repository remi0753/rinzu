#define _POSIX_C_SOURCE 200809L
#include "rinzu/standalone_interrupt.h"
void rinzu_standalone_interrupt_startup(void) {
  rinzu_standalone_interrupt_normalize();
}
