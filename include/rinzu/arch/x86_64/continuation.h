#ifndef RINZU_X86_64_CONTINUATION_H
#define RINZU_X86_64_CONTINUATION_H
#define RINZU_CONTEXT_RBX 0
#define RINZU_CONTEXT_RBP 8
#define RINZU_CONTEXT_R12 16
#define RINZU_CONTEXT_R13 24
#define RINZU_CONTEXT_R14 32
#define RINZU_CONTEXT_R15 40
#define RINZU_CONTEXT_RSP 48
#define RINZU_CONTEXT_IP 56
#define RINZU_CONTEXT_MXCSR 64
#define RINZU_CONTEXT_FCW 68

#ifndef __ASSEMBLER__
#include <asm/prctl.h>
#include <errno.h>
#include <sys/syscall.h>

#ifndef ARCH_SHSTK_STATUS
#define ARCH_SHSTK_STATUS 0x5005
#endif
#ifndef ARCH_SHSTK_SHSTK
#define ARCH_SHSTK_SHSTK (1UL << 0)
#endif

typedef struct {
  uint64_t rbx, rbp, r12, r13, r14, r15, sp, ip;
  uint32_t mxcsr;
  uint16_t fcw;
} rinzu_continuation_registers_t;
_Static_assert(offsetof(rinzu_continuation_registers_t, rbx) == RINZU_CONTEXT_RBX &&
                   offsetof(rinzu_continuation_registers_t, rbp) == RINZU_CONTEXT_RBP &&
                   offsetof(rinzu_continuation_registers_t, r12) == RINZU_CONTEXT_R12 &&
                   offsetof(rinzu_continuation_registers_t, r13) == RINZU_CONTEXT_R13 &&
                   offsetof(rinzu_continuation_registers_t, r14) == RINZU_CONTEXT_R14 &&
                   offsetof(rinzu_continuation_registers_t, r15) == RINZU_CONTEXT_R15 &&
                   offsetof(rinzu_continuation_registers_t, sp) == RINZU_CONTEXT_RSP &&
                   offsetof(rinzu_continuation_registers_t, ip) == RINZU_CONTEXT_IP &&
                   offsetof(rinzu_continuation_registers_t, mxcsr) == RINZU_CONTEXT_MXCSR &&
                   offsetof(rinzu_continuation_registers_t, fcw) == RINZU_CONTEXT_FCW,
               "SysV continuation register layout");

void rinzu_continuation_bootstrap(void);
static inline void rinzu_continuation_arch_check(void) {
  unsigned long features = 0;
  if (syscall(SYS_arch_prctl, ARCH_SHSTK_STATUS, &features) == 0) {
    if (features & ARCH_SHSTK_SHSTK) {
      static const char message[] = "rinzu: async AOT requires CET shadow stacks to be disabled\n";
      (void)write(STDERR_FILENO, message, sizeof message - 1);
      _Exit(78);
    }
  } else if (errno != EINVAL && errno != ENOSYS && errno != EOPNOTSUPP) {
    rinzu_failure_fatal();
  }
}
static inline void rinzu_continuation_arch_init(rinzu_continuation_registers_t *registers,
                                                void (*run)(void *), void *context) {
  registers->r12 = (uintptr_t)context;
  registers->r13 = (uintptr_t)run;
  registers->ip = (uintptr_t)rinzu_continuation_bootstrap;
  __asm__ volatile("stmxcsr %0" : "=m"(registers->mxcsr));
  __asm__ volatile("fnstcw %0" : "=m"(registers->fcw));
}
#endif
#endif
