// Version 1: the immediately preceding RW page has one control slot per entry.
// x0-x7, floating registers and stack arguments are untouched by the template.
.text
.p2align 14
.globl _rinzu_callback_template
_rinzu_callback_template:
.rept 512
  adr x16, . - 16384
  ldp x16, x17, [x16]
  br x17
  .space 20
.endr
.globl _rinzu_callback_template_end
_rinzu_callback_template_end:

.globl _rinzu_callback_pointer_u64_bridge
_rinzu_callback_pointer_u64_bridge:
  mov x1, x16
  b _rinzu_callback_pointer_u64_dispatch

// Preserve both independent register banks and the original Darwin stack area.
// The generic dispatcher returns raw bits; populate both physical result banks.
.p2align 2
.globl _rinzu_callback_frame_bridge
_rinzu_callback_frame_bridge:
  stp x29, x30, [sp, #-160]!
  mov x29, sp
  stp x0, x1, [sp, #16]
  stp x2, x3, [sp, #32]
  stp x4, x5, [sp, #48]
  stp x6, x7, [sp, #64]
  stp d0, d1, [sp, #80]
  stp d2, d3, [sp, #96]
  stp d4, d5, [sp, #112]
  stp d6, d7, [sp, #128]
  add x2, sp, #160
  str x2, [sp, #144]
  mov x0, x16
  add x1, sp, #16
  bl _rinzu_callback_frame_dispatch
  fmov d0, x0
  ldp x29, x30, [sp], #160
  ret
