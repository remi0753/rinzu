#ifndef RINZU_TESTING_CASES_H
#define RINZU_TESTING_CASES_H

#include <stddef.h>
#include <stdint.h>

int64_t rinzu_testing_case_begin(const uint8_t *name, size_t length);
void rinzu_testing_case_error(const uint8_t *name, size_t name_length, const uint8_t *message,
                              size_t message_length);
void rinzu_testing_case_end(int64_t failed);

#endif
