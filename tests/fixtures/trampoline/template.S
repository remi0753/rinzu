// Precompiled instructions only. Control slots occupy the preceding VM page.
.text
.p2align 14
.globl _rinzu_template
_rinzu_template:
.rept 512
  adr x16, . - 16384
  ldp x16, x17, [x16]
  br x17
  .space 20
.endr
.globl _rinzu_template_end
_rinzu_template_end:

.globl _rinzu_integer_bridge
_rinzu_integer_bridge:
  mov x2, x16
  b _rinzu_integer_handler
.globl _rinzu_pointer_bridge
_rinzu_pointer_bridge:
  mov x2, x16
  b _rinzu_pointer_handler
.globl _rinzu_float_bridge
_rinzu_float_bridge:
  mov x0, x16
  b _rinzu_float_handler

// Process-lifetime common admission gate, before signature-specific handlers.
.globl _rinzu_gate_bridge
_rinzu_gate_bridge:
  ldr x17, [x16, #8]
  cbnz x17, Lclosed
  ldr x17, [x16, #16]
  br x17
Lclosed:
  mov x0, #86
  b __Exit
