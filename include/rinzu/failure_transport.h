#ifndef RINZU_FAILURE_TRANSPORT_H
#define RINZU_FAILURE_TRANSPORT_H
#include "rinzu/failure.h"
#include "rinzu/scalar_transport.h"
#include "rinzu/key.h"
/* Diagnostic, cleanup list, then ordered trace entries. A trace entry contains
   its diagnostic index and an inner-first list of copied function/position data. */
static inline size_t rinzu_failure_trace_size(const rinzu_error_trace_t *trace) {
  size_t size = 16;
  for (size_t i = 0; i < trace->count; ++i) {
    size_t length = trace->frames[i].function_length;
    if (length > SIZE_MAX - 25 || size > SIZE_MAX - 25 - length)
      rinzu_failure_fatal();
    size += 25 + length;
  }
  return size;
}
static inline size_t rinzu_failure_transfer_size(const rinzu_failure_t *failure) {
  size_t size = 34;
  if (failure->primary_trace) {
    size_t trace = rinzu_failure_trace_size(failure->primary_trace);
    if (trace > SIZE_MAX - size)
      rinzu_failure_fatal();
    size += trace;
  }
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    size_t trace = node->trace ? rinzu_failure_trace_size(node->trace) : 0;
    if (trace > SIZE_MAX - 18 || size > SIZE_MAX - 18 - trace)
      rinzu_failure_fatal();
    size += 18 + trace;
  }
  return size;
}
static inline int rinzu_failure_transfer_diagnostic(unsigned char *bytes, int64_t cause,
                                                    rinzu_source_position_t source) {
  if (cause < 1 || cause > 16 || source.present > 1 ||
      (source.present && source.start_byte > source.end_byte))
    return RINZU_TRANSFER_ENCODING;
  const unsigned kinds[] = {RINZU_SCALAR_U8, RINZU_SCALAR_BOOL, RINZU_SCALAR_U64, RINZU_SCALAR_U32,
                            RINZU_SCALAR_U32};
  const uint64_t slots[] = {(uint64_t)cause, source.present, source.present ? source.file : 0,
                            source.present ? source.start_byte : 0,
                            source.present ? source.end_byte : 0};
  return rinzu_transfer_encode_record(kinds, slots, 5, bytes, 18);
}
static inline int rinzu_failure_transfer_read(const unsigned char *bytes, int64_t *cause,
                                              rinzu_source_position_t *source) {
  const unsigned kinds[] = {RINZU_SCALAR_U8, RINZU_SCALAR_BOOL, RINZU_SCALAR_U64, RINZU_SCALAR_U32,
                            RINZU_SCALAR_U32};
  uint64_t slots[5];
  if (rinzu_transfer_decode_record(kinds, 5, bytes, 18, slots) || slots[0] < 1 || slots[0] > 16 ||
      (slots[1] && slots[3] > slots[4]) || (!slots[1] && (slots[2] || slots[3] || slots[4])))
    return RINZU_TRANSFER_ENCODING;
  *cause = (int64_t)slots[0];
  *source = (rinzu_source_position_t){slots[2], (uint32_t)slots[3], (uint32_t)slots[4],
                                      (unsigned)slots[1]};
  return RINZU_TRANSFER_OK;
}
static inline void rinzu_failure_transfer_u64(unsigned char *bytes, uint64_t value) {
  if (rinzu_transfer_encode_scalar(RINZU_SCALAR_U64, value, bytes, 8))
    rinzu_failure_fatal();
}
static inline int rinzu_failure_transfer_trace_valid(const rinzu_error_trace_t *trace) {
  unsigned char source[18];
  for (size_t i = 0; i < trace->count; ++i) {
    const rinzu_error_trace_frame_t *frame = &trace->frames[i];
    if (!rinzu_utf8_valid((const unsigned char *)frame->function, frame->function_length) ||
        rinzu_failure_transfer_diagnostic(source, 15, frame->source))
      return RINZU_TRANSFER_ENCODING;
  }
  return RINZU_TRANSFER_OK;
}
static inline void rinzu_failure_transfer_trace_write(unsigned char *bytes, size_t *offset,
                                                      size_t index,
                                                      const rinzu_error_trace_t *trace) {
  rinzu_failure_transfer_u64(bytes + *offset, index);
  *offset += 8;
  rinzu_failure_transfer_u64(bytes + *offset, trace->count);
  *offset += 8;
  for (size_t i = 0; i < trace->count; ++i) {
    const rinzu_error_trace_frame_t *frame = &trace->frames[i];
    rinzu_failure_transfer_u64(bytes + *offset, frame->function_length);
    *offset += 8;
    if (frame->function_length)
      memcpy(bytes + *offset, frame->function, frame->function_length);
    *offset += frame->function_length;
    unsigned char source[18];
    if (rinzu_failure_transfer_diagnostic(source, 15, frame->source))
      rinzu_failure_fatal();
    memcpy(bytes + *offset, source + 1, 17);
    *offset += 17;
  }
}
static inline int rinzu_failure_transfer_encode(const rinzu_failure_t *failure,
                                                unsigned char *bytes, size_t length) {
  if (failure->primary_error)
    return RINZU_TRANSFER_ENCODING;
  unsigned char scratch[18];
  if (rinzu_failure_transfer_diagnostic(scratch, failure->primary, failure->primary_source) ||
      (failure->primary_trace && rinzu_failure_transfer_trace_valid(failure->primary_trace)))
    return RINZU_TRANSFER_ENCODING;
  size_t count = 0, traces = !!failure->primary_trace;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    if (node->error || rinzu_failure_transfer_diagnostic(scratch, node->cause, node->source) ||
        (node->trace && rinzu_failure_transfer_trace_valid(node->trace)))
      return RINZU_TRANSFER_ENCODING;
    count++;
    traces += !!node->trace;
  }
  if (!bytes || length != rinzu_failure_transfer_size(failure))
    return RINZU_TRANSFER_LENGTH;
  rinzu_failure_transfer_diagnostic(bytes, failure->primary, failure->primary_source);
  rinzu_failure_transfer_u64(bytes + 18, count);
  size_t offset = 26;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    rinzu_failure_transfer_diagnostic(bytes + offset, node->cause, node->source);
    offset += 18;
  }
  rinzu_failure_transfer_u64(bytes + offset, traces);
  offset += 8;
  if (failure->primary_trace)
    rinzu_failure_transfer_trace_write(bytes, &offset, 0, failure->primary_trace);
  size_t index = 1;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next, ++index)
    if (node->trace)
      rinzu_failure_transfer_trace_write(bytes, &offset, index, node->trace);
  return offset == length ? RINZU_TRANSFER_OK : RINZU_TRANSFER_LENGTH;
}
static inline int rinzu_failure_transfer_take_u64(const unsigned char *bytes, size_t length,
                                                  size_t *offset, uint64_t *value) {
  if (*offset > length || length - *offset < 8)
    return RINZU_TRANSFER_LENGTH;
  rinzu_scalar_result_t scalar = rinzu_transfer_decode_scalar(RINZU_SCALAR_U64, bytes + *offset, 8);
  if (scalar.status)
    return RINZU_TRANSFER_ENCODING;
  *value = scalar.bits;
  *offset += 8;
  return RINZU_TRANSFER_OK;
}
/* First pass validates every span/index/UTF-8 value without allocating or
   modifying the receiver. The second pass copies the immutable metadata. */
static inline int rinzu_failure_transfer_parse(const unsigned char *bytes, size_t length,
                                               rinzu_failure_t *output) {
  if (!bytes || length < 34)
    return RINZU_TRANSFER_LENGTH;
  uint64_t count = 0;
  size_t offset = 18;
  int status = rinzu_failure_transfer_take_u64(bytes, length, &offset, &count);
  if (status)
    return status;
  if (count > (length - 34) / 18)
    return RINZU_TRANSFER_LENGTH;
  int64_t cause = 0;
  rinzu_source_position_t source = {0};
  if (rinzu_failure_transfer_read(bytes, &cause, &source))
    return RINZU_TRANSFER_ENCODING;
  if (output) {
    output->primary = cause;
    output->primary_source = source;
  }
  for (uint64_t i = 0; i < count; ++i) {
    if (rinzu_failure_transfer_read(bytes + offset, &cause, &source))
      return RINZU_TRANSFER_ENCODING;
    offset += 18;
    if (output) {
      rinzu_diagnostic_node_t *node = rinzu_failure_allocate(sizeof(*node));
      node->cause = cause;
      node->source = source;
      if (output->tail)
        output->tail->next = node;
      else
        output->head = node;
      output->tail = node;
    }
  }
  uint64_t traces = 0;
  status = rinzu_failure_transfer_take_u64(bytes, length, &offset, &traces);
  if (status)
    return status;
  if (traces > count + 1)
    return RINZU_TRANSFER_ENCODING;
  uint64_t previous = 0, node_index = 1;
  rinzu_diagnostic_node_t *node = output ? output->head : NULL;
  for (uint64_t i = 0; i < traces; ++i) {
    uint64_t index = 0, frames_count = 0;
    status = rinzu_failure_transfer_take_u64(bytes, length, &offset, &index);
    if (!status)
      status = rinzu_failure_transfer_take_u64(bytes, length, &offset, &frames_count);
    if (status)
      return status;
    if (index > count || (i && index <= previous))
      return RINZU_TRANSFER_ENCODING;
    if (frames_count > (length - offset) / 25 ||
        frames_count > SIZE_MAX / sizeof(rinzu_error_trace_frame_t))
      return RINZU_TRANSFER_LENGTH;
    previous = index;
    rinzu_error_trace_frame_t *frames =
        output && frames_count ? rinzu_failure_allocate((size_t)frames_count * sizeof(*frames))
                               : NULL;
    for (uint64_t j = 0; j < frames_count; ++j) {
      uint64_t name_length = 0;
      status = rinzu_failure_transfer_take_u64(bytes, length, &offset, &name_length);
      if (status || name_length > length - offset || length - offset - (size_t)name_length < 17) {
        free(frames);
        return status ? status : RINZU_TRANSFER_LENGTH;
      }
      const unsigned char *name = bytes + offset;
      if (!rinzu_utf8_valid(name, (size_t)name_length)) {
        free(frames);
        return RINZU_TRANSFER_ENCODING;
      }
      offset += (size_t)name_length;
      unsigned char diagnostic[18] = {15};
      memcpy(diagnostic + 1, bytes + offset, 17);
      offset += 17;
      if (rinzu_failure_transfer_read(diagnostic, &cause, &source)) {
        free(frames);
        return RINZU_TRANSFER_ENCODING;
      }
      if (frames)
        frames[j] = (rinzu_error_trace_frame_t){(const char *)name, (size_t)name_length, source};
    }
    if (output) {
      rinzu_error_trace_t *trace = rinzu_error_trace_capture(frames, (size_t)frames_count);
      if (!index)
        output->primary_trace = trace;
      else {
        while (node_index < index && node) {
          node = node->next;
          node_index++;
        }
        if (!node)
          rinzu_failure_fatal();
        node->trace = trace;
      }
    }
    free(frames);
  }
  return offset == length ? RINZU_TRANSFER_OK : RINZU_TRANSFER_LENGTH;
}
static inline int rinzu_failure_transfer_decode(const unsigned char *bytes, size_t length,
                                                rinzu_failure_t *output) {
  if (!output)
    return RINZU_TRANSFER_LENGTH;
  if (output->primary || output->primary_error || output->primary_trace || output->head ||
      output->tail)
    return RINZU_TRANSFER_ENCODING;
  int status = rinzu_failure_transfer_parse(bytes, length, NULL);
  if (status)
    return status;
  rinzu_failure_t decoded = {0};
  status = rinzu_failure_transfer_parse(bytes, length, &decoded);
  if (status) {
    rinzu_failure_release(decoded);
    return status;
  }
  *output = decoded;
  return RINZU_TRANSFER_OK;
}
#endif
