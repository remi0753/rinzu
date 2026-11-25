#ifndef RINZU_RUNTIME_STORAGE_H
#define RINZU_RUNTIME_STORAGE_H
/* A monolithic runtime owns its state locally. Split translation units select
   one implementation and refer to those image-private definitions elsewhere. */
#ifndef RINZU_RUNTIME_GLOBAL
#define RINZU_RUNTIME_GLOBAL static
#define RINZU_RUNTIME_INITIALIZER(...) = __VA_ARGS__
#endif
#endif
