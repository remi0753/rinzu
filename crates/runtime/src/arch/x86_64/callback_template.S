#include "rinzu/arch/x86_64/callback.h"

// The preceding RW page contains one non-reusable control slot per entry.
.text
.balign RINZU_CALLBACK_PAGE
.globl rinzu_callback_template
.hidden rinzu_callback_template
rinzu_callback_template:
.rept RINZU_CALLBACK_PAGE / RINZU_CALLBACK_STRIDE
  endbr64
  leaq . - RINZU_CALLBACK_PAGE - 4(%rip), %r11
  movq (%r11), %r10
  jmp *8(%r11)
  .balign RINZU_CALLBACK_STRIDE, 0x90
.endr
.globl rinzu_callback_template_end
.hidden rinzu_callback_template_end
rinzu_callback_template_end:

.globl rinzu_callback_pointer_u64_bridge
.hidden rinzu_callback_pointer_u64_bridge
.type rinzu_callback_pointer_u64_bridge,@function
rinzu_callback_pointer_u64_bridge:
  endbr64
  movq %r10, %rsi
  jmp rinzu_callback_pointer_u64_dispatch
.size rinzu_callback_pointer_u64_bridge, .-rinzu_callback_pointer_u64_bridge

// Capture both argument banks before entering C. The return address occupies
// the first word of the original stack. The first spilled argument follows it.
.p2align 4
.globl rinzu_callback_frame_bridge
.hidden rinzu_callback_frame_bridge
.type rinzu_callback_frame_bridge,@function
rinzu_callback_frame_bridge:
  endbr64
  subq $136, %rsp
  movq %rdi, 0(%rsp)
  movq %rsi, 8(%rsp)
  movq %rdx, 16(%rsp)
  movq %rcx, 24(%rsp)
  movq %r8, 32(%rsp)
  movq %r9, 40(%rsp)
  movq %xmm0, 64(%rsp)
  movq %xmm1, 72(%rsp)
  movq %xmm2, 80(%rsp)
  movq %xmm3, 88(%rsp)
  movq %xmm4, 96(%rsp)
  movq %xmm5, 104(%rsp)
  movq %xmm6, 112(%rsp)
  movq %xmm7, 120(%rsp)
  leaq 144(%rsp), %rax
  movq %rax, 128(%rsp)
  movq %r10, %rdi
  movq %rsp, %rsi
  call rinzu_callback_frame_dispatch
  movq %rax, %xmm0
  addq $136, %rsp
  ret
.size rinzu_callback_frame_bridge, .-rinzu_callback_frame_bridge
.section .note.GNU-stack,"",@progbits
