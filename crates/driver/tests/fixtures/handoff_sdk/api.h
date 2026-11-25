#ifndef RINZU_HANDOFF_API_H
#define RINZU_HANDOFF_API_H
#include <stddef.h>
typedef struct handoff_completion HandoffCompletion;
HandoffCompletion *handoff_start(size_t (*callback)(void));
int handoff_finish(void);
int handoff_controlled(void);
#endif
