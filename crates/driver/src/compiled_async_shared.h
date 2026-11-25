#include <dlfcn.h>
#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
int rinzu_jit_printf(const char *, ...);
extern uint64_t (*rinzu_jit_context_identity)(void);
#define printf rinzu_jit_printf
#define RINZU_CONTEXT_IDENTITY()                                                                   \
  (rinzu_jit_context_identity ? rinzu_jit_context_identity() : (_Exit(86), UINT64_C(0)))
#ifdef RINZU_RUNTIME_IMPLEMENTATION
#define RINZU_RUNTIME_GLOBAL __attribute__((visibility("hidden")))
#define RINZU_RUNTIME_INITIALIZER(...) = __VA_ARGS__
#else
#define RINZU_RUNTIME_GLOBAL extern __attribute__((visibility("hidden")))
#define RINZU_RUNTIME_INITIALIZER(...)
#endif
#include "rinzu/runtime.h"
#include "rinzu/display.h"
#include "rinzu/failure.h"
#include "rinzu/error_groups.h"
#include "rinzu/managed_composite.h"
#include "rinzu/native_record.h"
#include "rinzu/native_userdata.h"
#include "rinzu/owner_transition.h"
#include "rinzu/source_async.h"
#include "rinzu/continuation.h"
#include "rinzu/native_input.h"
#include "rinzu/native_opaque.h"
#include "rinzu/callback_return.h"
#include "rinzu/callback_consumption.h"
#include "rinzu/foreign_queue.h"
#ifdef RINZU_AOT_HAS_SUPPLANT
#include "rinzu/supplant.h"
#include "rinzu/owner_dispatcher.h"
#endif

#ifdef RINZU_AOT_HAS_JSON
#include "rinzu/json.h"
#endif
