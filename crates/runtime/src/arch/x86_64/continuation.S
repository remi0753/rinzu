#include "rinzu/arch/x86_64/continuation.h"

.text
.p2align 4
.globl rinzu_continuation_switch
.hidden rinzu_continuation_switch
.type rinzu_continuation_switch,@function
rinzu_continuation_switch:
  movq %rbx, RINZU_CONTEXT_RBX(%rdi)
  movq %rbp, RINZU_CONTEXT_RBP(%rdi)
  movq %r12, RINZU_CONTEXT_R12(%rdi)
  movq %r13, RINZU_CONTEXT_R13(%rdi)
  movq %r14, RINZU_CONTEXT_R14(%rdi)
  movq %r15, RINZU_CONTEXT_R15(%rdi)
  leaq 8(%rsp), %rax
  movq %rax, RINZU_CONTEXT_RSP(%rdi)
  movq (%rsp), %rax
  movq %rax, RINZU_CONTEXT_IP(%rdi)
  stmxcsr RINZU_CONTEXT_MXCSR(%rdi)
  fnstcw RINZU_CONTEXT_FCW(%rdi)
  movq RINZU_CONTEXT_RBX(%rsi), %rbx
  movq RINZU_CONTEXT_RBP(%rsi), %rbp
  movq RINZU_CONTEXT_R12(%rsi), %r12
  movq RINZU_CONTEXT_R13(%rsi), %r13
  movq RINZU_CONTEXT_R14(%rsi), %r14
  movq RINZU_CONTEXT_R15(%rsi), %r15
  ldmxcsr RINZU_CONTEXT_MXCSR(%rsi)
  fldcw RINZU_CONTEXT_FCW(%rsi)
  movq RINZU_CONTEXT_RSP(%rsi), %rsp
  pushq RINZU_CONTEXT_IP(%rsi)
  ret
.size rinzu_continuation_switch, .-rinzu_continuation_switch

.p2align 4
.globl rinzu_continuation_bootstrap
.hidden rinzu_continuation_bootstrap
.type rinzu_continuation_bootstrap,@function
rinzu_continuation_bootstrap:
  # The new stack is 16-byte aligned here, before the first call.
  movq %r12, %rdi
  call *%r13
  ud2
.size rinzu_continuation_bootstrap, .-rinzu_continuation_bootstrap
.section .note.GNU-stack,"",@progbits
