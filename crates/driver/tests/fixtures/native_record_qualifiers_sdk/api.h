#ifndef RINZU_RECORD_QUALIFIERS_FIXTURE_H
#define RINZU_RECORD_QUALIFIERS_FIXTURE_H
#include <stdint.h>
typedef struct ForeignPoint { int64_t x; } ForeignPoint;
typedef const ForeignPoint *Read;
typedef ForeignPoint *Write;
typedef int64_t (*Handler)(Write,Read);
Read qualifier_aliases(Read,Write,Handler);
int64_t qualifier_change(Write,int64_t);
#endif
