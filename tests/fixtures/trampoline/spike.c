#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define PAGE 16384
#define STRIDE 32
#define SLOTS (PAGE / STRIDE)
extern char rinzu_template[], rinzu_template_end[];
extern void rinzu_gate_bridge(void);
extern void rinzu_integer_bridge(void), rinzu_pointer_bridge(void), rinzu_float_bridge(void);
typedef struct {
  uintptr_t identity;
  uintptr_t closed;
  uintptr_t bridge;
} Control;
typedef struct {
  Control *context;
  uintptr_t gate;
  uintptr_t reserved[2];
} Slot;
typedef struct Block {
  mach_vm_address_t base;
  struct Block *next;
} Block;
static Block *blocks;
static size_t identities, block_count;
static const char *mode;
static void require(int condition, const char *message) {
  if (!condition) {
    fprintf(stderr, "image/ABI: %s\n", message);
    exit(78);
  }
}
static void vm_check(kern_return_t result, const char *operation) {
  if (result == KERN_SUCCESS)
    return;
  fprintf(stderr, "%s: %s (%d), category=%s\n", operation, mach_error_string(result), result,
          result == KERN_RESOURCE_SHORTAGE || result == KERN_NO_SPACE ? "resource"
                                                                      : "permission/image");
  _Exit(result == KERN_RESOURCE_SHORTAGE || result == KERN_NO_SPACE ? 71 : 78);
}
static Control *gate(Control *control) {
  if (control->closed)
    _Exit(86);
  return control;
}
int64_t rinzu_integer_handler(int64_t a, int64_t b, Control *control) {
  return a + b + (int64_t)gate(control)->identity;
}
void *rinzu_pointer_handler(void *data, size_t length, Control *control) {
  gate(control);
  require(length == 4, "Bytes(4) length/userdata");
  return data;
}
double rinzu_float_handler(double a, double b, Control *control) {
  return a + b + (double)gate(control)->identity;
}
static Block *grow(void) {
  if (mode && strcmp(mode, "resource") == 0)
    vm_check(KERN_RESOURCE_SHORTAGE, "injected reserve");
  Block *block = calloc(1, sizeof(*block));
  if (!block)
    _Exit(71);
  vm_check(mach_vm_allocate(mach_task_self(), &block->base, PAGE * 2, VM_FLAGS_ANYWHERE),
           "reserve");
  mach_vm_address_t target = block->base + PAGE;
  vm_prot_t current, maximum;
  vm_check(mach_vm_remap(mach_task_self(), &target, PAGE, 0, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
                         mach_task_self(), (mach_vm_address_t)rinzu_template, FALSE, &current,
                         &maximum, VM_INHERIT_NONE),
           "remap signed template");
  require(target == block->base + PAGE, "RX/control adjacency");
  require((current & (VM_PROT_READ | VM_PROT_EXECUTE)) == (VM_PROT_READ | VM_PROT_EXECUTE) &&
              !(current & VM_PROT_WRITE),
          "template RX protection");
  vm_check(
      mach_vm_protect(mach_task_self(), block->base, PAGE, FALSE, VM_PROT_READ | VM_PROT_WRITE),
      "control RW protection");
  mach_vm_address_t region = block->base;
  mach_vm_size_t region_size = 0;
  vm_region_basic_info_data_64_t info;
  mach_msg_type_number_t info_count = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t object = MACH_PORT_NULL;
  vm_check(mach_vm_region(mach_task_self(), &region, &region_size, VM_REGION_BASIC_INFO_64,
                          (vm_region_info_t)&info, &info_count, &object),
           "inspect control");
  require(region == block->base && region_size >= PAGE &&
              info.protection == (VM_PROT_READ | VM_PROT_WRITE),
          "control RW/non-executable");
  if (object != MACH_PORT_NULL)
    mach_port_deallocate(mach_task_self(), object);
  block->next = blocks;
  blocks = block;
  block_count++;
  return block;
}
int main(int argc, char **argv) {
  mode = argc > 1 ? argv[1] : NULL;
  if (mode && strcmp(mode, "image") == 0)
    require(0, "injected incompatible template version");
  if (mode && strcmp(mode, "permission") == 0)
    vm_check(KERN_PROTECTION_FAILURE, "injected remap protection");
  require(getpagesize() == PAGE, "target page size");
  require((uintptr_t)rinzu_template % PAGE == 0, "template alignment");
  require(rinzu_template_end - rinzu_template == PAGE, "template size/version 1");
  require(sizeof(Slot) == STRIDE, "control stride");
  // The fixture count is a test boundary, not an allocator capacity.
  size_t count = SLOTS * 2 + 7;
  void **entries = calloc(count, sizeof(*entries));
  if (!entries)
    _Exit(71);
  Block *block = NULL;
  for (size_t i = 0; i < count; i++) {
    if (i % SLOTS == 0)
      block = grow();
    Slot *slot = (Slot *)(block->base + (i % SLOTS) * STRIDE);
    Control *control = calloc(1, sizeof(*control));
    if (!control)
      _Exit(71);
    slot->context = control;
    slot->gate = (uintptr_t)rinzu_gate_bridge;
    control->identity = ++identities;
    control->bridge = (uintptr_t)(i % 3 == 0   ? rinzu_integer_bridge
                                  : i % 3 == 1 ? rinzu_pointer_bridge
                                               : rinzu_float_bridge);
    entries[i] = (void *)(block->base + PAGE + (i % SLOTS) * STRIDE);
    for (size_t j = 0; j < i; j++)
      require(entries[j] != entries[i], "entry identity reused");
  }
  if (mode && strcmp(mode, "closed") == 0) {
    // Retain admission information while releasing the handler connection.
    Slot *slot = (Slot *)((uintptr_t)entries[0] - PAGE);
    slot->context->closed = 1;
    slot->context->bridge = 0;
    ((int64_t (*)(int64_t, int64_t))entries[0])(0, 0);
    require(0, "closed gate returned");
  }
  // Repeat old entries after all growth; argument registers and results survive.
  unsigned char bytes[4] = {1, 2, 3, 4};
  for (int repeat = 0; repeat < 3; repeat++) {
    for (size_t i = 0; i < count; i++) {
      if (i % 3 == 0)
        require(((int64_t (*)(int64_t, int64_t))entries[i])(-17, 29) == 12 + (int64_t)i + 1,
                "integer ABI");
      else if (i % 3 == 1)
        require(((void *(*)(void *, size_t))entries[i])(bytes, 4) == bytes, "pointer ABI/userdata");
      else
        require(((double (*)(double, double))entries[i])(1.25, -0.5) == 0.75 + (double)i + 1,
                "float ABI");
    }
  }
  require(identities == count && block_count == 3, "repeat calls allocated entries");
  printf("PASS template_version=1 page=%d stride=%d identities=%zu blocks=%zu virtual_bytes=%zu "
         "control_bytes=%zu retained_until_process_exit=true\n",
         PAGE, STRIDE, identities, block_count, block_count * PAGE * 2,
         block_count * PAGE + count * sizeof(Control));
  return 0;
}
