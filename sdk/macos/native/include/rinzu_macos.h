#ifndef RINZU_MACOS_H
#define RINZU_MACOS_H
#include <stddef.h>
#include <stdint.h>
#include "rinzu/native_bytes.h"

struct MacRect {
  double x, y, width, height;
};
struct MacColor {
  double red, green, blue, alpha;
};
struct MacTextureInfo {
  int64_t width, height, format, usage, bytes;
};
struct MacVertex2D {
  double x, y, u, v, red, green, blue, alpha;
};
struct MacTransform2D {
  double a, b, c, d, tx, ty;
};
struct MacQuad2D {
  double x, y, width, height, u, v, uvWidth, uvHeight;
  double red, green, blue, alpha, a, b, c, d, tx, ty;
};
struct MacScissor {
  int64_t x, y, width, height;
};
struct MacTextMetrics {
  double width, height, baseline, ascent, descent, leading;
  double left, top, rasterWidth, rasterHeight;
  int64_t lines;
};
struct MacFrameInfo {
  int64_t sequence, width, height;
  double scale, time, delta;
};
struct MacFrameStatistics {
  int64_t requests, coalesced, ticks, retries;
  _Bool pending, visible, animating, stopped;
};
struct MacEvent {
  int64_t kind, windowId, viewId, sequence;
  double x, y, width, height, scale;
  int64_t keyCode, modifiers;
  int64_t action, selectionStart, selectionEnd, flags;
  double value, deltaX, deltaY;
};
struct MacStatistics {
  int64_t sessions, windows, views, devices, buffers, libraries, pipelines, commands;
  int64_t bufferBytes, retainedControls;
  int64_t queuedEvents, eventPayloadBytes;
  int64_t snapshotBytes;
  int64_t sheets, sheetBytes;
  int64_t textures, samplers, textureBytes;
  int64_t fonts, textLayouts, textBytes;
  int64_t frameClocks, frameTimers, frameWaiters;
  int64_t dataRows, dataBytes, dataUpdates;
};
struct MacSelection {
  int64_t start, end;
};
struct MacWidgetState {
  double value;
  _Bool enabled, hidden;
};
struct MacDataState {
  int64_t rows, visibleRows, cellViews, bytes, revision;
  int64_t selectedId, selectedRow, firstVisible, lastVisible, pendingRows;
};
int64_t rinzu_macos_data_create(void *window, int64_t kind, int64_t columns,
                                const struct MacRect *rect, int64_t action);
int32_t rinzu_macos_data_column(void *view, int64_t column, const uint8_t *title, size_t length,
                                double width, _Bool sortable);
int32_t rinzu_macos_data_begin(void *view, int64_t token);
_Bool rinzu_macos_data_owned(void *view, int64_t token);
int32_t rinzu_macos_data_append(void *view, int64_t token, int64_t id, int64_t parent,
                                const uint8_t *cells, size_t length);
int32_t rinzu_macos_data_commit(void *view, int64_t token);
int32_t rinzu_macos_data_cancel(void *view, int64_t token);
int32_t rinzu_macos_data_state(void *view, struct MacDataState *output);
int32_t rinzu_macos_data_select(void *view, int64_t id, _Bool reveal);
int32_t rinzu_macos_data_scroll(void *view, int64_t id);
int32_t rinzu_macos_data_expand(void *view, int64_t id, _Bool expanded);
int32_t rinzu_macos_data_expanded(void *view, int64_t id);
int32_t rinzu_macos_data_sort(void *view, int64_t column, _Bool ascending);
int32_t rinzu_macos_data_cell(void *view, int64_t token, int64_t id, int64_t column);
int32_t rinzu_macos_view_accessibility(void *view, const uint8_t *label, size_t label_length,
                                       const uint8_t *help, size_t help_length,
                                       const uint8_t *identifier, size_t identifier_length);
int32_t rinzu_macos_view_tooltip(void *view, const uint8_t *text, size_t length);
int32_t rinzu_macos_window_theme(void *window, int64_t theme);
int32_t rinzu_macos_window_effective_theme(void *window);

int64_t rinzu_macos_open(void);
void *rinzu_macos_session(int64_t id);
int32_t rinzu_macos_close(void *session);
int32_t rinzu_macos_prepare_close(void *session);
void *rinzu_macos_owner_session(void);
_Bool rinzu_macos_has_owner_session(void);
const rinzu_native_byte_array_t *rinzu_macos_diagnostic(void);
int64_t rinzu_macos_resource_root_size(void *session);
const rinzu_native_byte_array_t *rinzu_macos_resource_root_bytes(void *session);
int32_t rinzu_macos_resource_relative(const uint8_t *path, size_t length);
int32_t rinzu_macos_statistics(struct MacStatistics *output);
int32_t rinzu_macos_run(void *session, void *context,
                        void (*handler)(void *, const struct MacEvent *));
int32_t rinzu_macos_stop(void *session);
int32_t rinzu_macos_run_events(void *session);
int32_t rinzu_macos_stream_state(void *session);
int32_t rinzu_macos_prepare_events(void *session);
int32_t rinzu_macos_request_stop(void *session);
_Bool rinzu_macos_has_run(void *session);
_Bool rinzu_macos_running(void *session);
int32_t rinzu_macos_run_wait_begin(void *session, int64_t token);
_Bool rinzu_macos_run_wait_owned(void *session, int64_t token);
void *rinzu_macos_run_completion(void *session, int64_t token);
int32_t rinzu_macos_run_wait_finish(void *session, int64_t token);
int32_t rinzu_macos_event_wait_begin(void *session, int64_t token);
_Bool rinzu_macos_event_wait_owned(void *session, int64_t token);
void *rinzu_macos_event_completion(void *session, int64_t token);
int32_t rinzu_macos_event_take(void *session, int64_t token, struct MacEvent *output);
const rinzu_native_byte_array_t *rinzu_macos_event_bytes(void *session, int64_t token);
int32_t rinzu_macos_event_cancel(void *session, int64_t token);
int32_t rinzu_macos_event_finish(void *session, int64_t token);
int32_t rinzu_macos_scope_notify(void *session, int64_t window_id);
int64_t rinzu_macos_event_sequence(void *session);
int64_t rinzu_macos_window_scoped(void *session, int64_t token, const uint8_t *title, size_t length,
                                  double width, double height);
int32_t rinzu_macos_window_scope_close(void *session, int64_t token);
int32_t rinzu_macos_window_scope_prepare(void *session, int64_t token);
_Bool rinzu_macos_window_scope_has_command(void *session, int64_t token);
void *rinzu_macos_window_scope_command(void *session, int64_t token);

int64_t rinzu_macos_window_create(void *session, const uint8_t *title, size_t length, double width,
                                  double height);
void *rinzu_macos_window(int64_t id);
int64_t rinzu_macos_window_id(void *window);
int32_t rinzu_macos_window_show(void *window);
int32_t rinzu_macos_window_title(void *window, const uint8_t *title, size_t length);
int32_t rinzu_macos_window_size(void *window, double width, double height);
int32_t rinzu_macos_window_minimum_size(void *window, double width, double height);
int32_t rinzu_macos_window_state(void *window);
int32_t rinzu_macos_window_visibility(void *window, int32_t operation);
int32_t rinzu_macos_sheet_ready(void *window);
int32_t rinzu_macos_panel_begin(void *window, int64_t token, int32_t save, const uint8_t *title,
                                size_t title_length, const uint8_t *directory,
                                size_t directory_length, const uint8_t *name, size_t name_length);
int32_t rinzu_macos_alert_begin(void *window, int64_t token, const uint8_t *title,
                                size_t title_length, const uint8_t *message, size_t message_length,
                                const uint8_t *first, size_t first_length, const uint8_t *second,
                                size_t second_length, const uint8_t *third, size_t third_length);
_Bool rinzu_macos_sheet_owned(void *window, int64_t token);
void *rinzu_macos_sheet_completion(void *window, int64_t token);
int32_t rinzu_macos_sheet_result(void *window, int64_t token);
const rinzu_native_byte_array_t *rinzu_macos_sheet_bytes(void *window, int64_t token);
int32_t rinzu_macos_sheet_cancel(void *window, int64_t token);
int32_t rinzu_macos_sheet_finish(void *window, int64_t token);
int32_t rinzu_macos_sheet_dismiss(void *window);
int32_t rinzu_macos_sheet_stop(void *window);
int32_t rinzu_macos_window_destroy(void *window);
void *rinzu_macos_window_content(void *window);
int64_t rinzu_macos_view_create(void *session, const struct MacRect *rect);
void *rinzu_macos_view(int64_t id);
int64_t rinzu_macos_view_id(void *view);
int32_t rinzu_macos_view_frame(void *view, struct MacRect *rect);
int32_t rinzu_macos_view_set_frame(void *view, const struct MacRect *rect);
int32_t rinzu_macos_view_add(void *parent, void *child);
int32_t rinzu_macos_view_remove(void *view);
int32_t rinzu_macos_view_destroy(void *view);
int32_t rinzu_macos_view_request_draw(void *view);
int64_t rinzu_macos_widget_create(void *session, int32_t kind, const uint8_t *text, size_t length,
                                  const struct MacRect *rect, int64_t action);
int64_t rinzu_macos_widget_scoped(void *window, int32_t kind, const uint8_t *text, size_t length,
                                  const struct MacRect *rect, int64_t action);
int32_t rinzu_macos_widget_set_text(void *view, const uint8_t *text, size_t length);
int32_t rinzu_macos_widget_snapshot(void *view, int64_t token);
_Bool rinzu_macos_widget_snapshot_owned(void *view, int64_t token);
const rinzu_native_byte_array_t *rinzu_macos_widget_bytes(void *view, int64_t token);
int32_t rinzu_macos_widget_snapshot_finish(void *view, int64_t token);
int32_t rinzu_macos_widget_state(void *view, struct MacWidgetState *output);
int32_t rinzu_macos_widget_value(void *view, double value);
int32_t rinzu_macos_view_enabled(void *view, _Bool enabled);
int32_t rinzu_macos_view_hidden(void *view, _Bool hidden);
int32_t rinzu_macos_view_focus(void *view);
int32_t rinzu_macos_text_selection(void *view, struct MacSelection *output);
int32_t rinzu_macos_text_select(void *view, int64_t start, int64_t end);
int32_t rinzu_macos_text_composing(void *view);
int32_t rinzu_macos_scroll_content(void *scroll, void *view);
int32_t rinzu_macos_scroll_to(void *scroll, double x, double y);
int32_t rinzu_macos_scroll_rect(void *scroll, struct MacRect *output);
int32_t rinzu_macos_view_autoresizing(void *view, int64_t mask);
int64_t rinzu_macos_menu_create(void *window, const uint8_t *title, size_t length);
void *rinzu_macos_menu(int64_t id);
int32_t rinzu_macos_menu_item(void *menu, const uint8_t *title, size_t length, const uint8_t *key,
                              size_t key_length, int64_t modifiers, int64_t action,
                              int32_t standard);
int32_t rinzu_macos_menu_submenu(void *parent, void *child);
int32_t rinzu_macos_menu_separator(void *menu);
int32_t rinzu_macos_menu_state(void *menu, int64_t action, _Bool enabled, _Bool checked);
int32_t rinzu_macos_menu_destroy(void *menu);
int32_t rinzu_macos_window_menu(void *window, void *menu);
int32_t rinzu_macos_window_command(void *window, int32_t command);
int32_t rinzu_macos_clipboard_write(void *session, const uint8_t *text, size_t length);
int32_t rinzu_macos_clipboard_snapshot(void *session, int64_t token);
_Bool rinzu_macos_clipboard_owned(void *session, int64_t token);
const rinzu_native_byte_array_t *rinzu_macos_clipboard_bytes(void *session, int64_t token);
int32_t rinzu_macos_clipboard_finish(void *session, int64_t token);

_Bool rinzu_macos_metal_available(void);
int64_t rinzu_macos_device_create(void *session);
void *rinzu_macos_device(int64_t id);
int32_t rinzu_macos_device_destroy(void *device);
int64_t rinzu_macos_buffer_create(void *device, const uint8_t *bytes, size_t length);
int64_t rinzu_macos_buffer_allocate(void *device, int64_t length);
void *rinzu_macos_buffer(int64_t id);
int32_t rinzu_macos_buffer_write(void *buffer, int64_t offset, const uint8_t *bytes, size_t length);
int32_t rinzu_macos_buffer_read(void *buffer, int64_t offset, int64_t length);
const rinzu_native_byte_array_t *rinzu_macos_buffer_bytes(void *buffer);
int32_t rinzu_macos_buffer_destroy(void *buffer);
int32_t rinzu_macos_buffer_vertex2d(void *buffer, int64_t index, const struct MacVertex2D *vertex);
int32_t rinzu_macos_buffer_quad2d(void *buffer, int64_t index, const struct MacQuad2D *quad);
int32_t rinzu_macos_buffer_writable(void *buffer);
int64_t rinzu_macos_texture_create(void *device, int64_t width, int64_t height, int64_t format,
                                   int64_t usage);
int64_t rinzu_macos_texture_image(void *device, const uint8_t *encoded, size_t length);
void *rinzu_macos_texture(int64_t id);
int64_t rinzu_macos_texture_id(void *texture);
int32_t rinzu_macos_texture_info(void *texture, struct MacTextureInfo *output);
int32_t rinzu_macos_texture_write(void *texture, const struct MacScissor *region, int64_t row_bytes,
                                  const uint8_t *bytes, size_t length);
int32_t rinzu_macos_texture_read(void *texture, const struct MacScissor *region);
const rinzu_native_byte_array_t *rinzu_macos_texture_bytes(void *texture);
int32_t rinzu_macos_texture_destroy(void *texture);
int64_t rinzu_macos_sampler_create(void *device, int64_t filter, int64_t address);
void *rinzu_macos_sampler(int64_t id);
int32_t rinzu_macos_sampler_destroy(void *sampler);
int32_t rinzu_macos_rotation2d(double radians, struct MacTransform2D *output);
int32_t rinzu_macos_surface_size(void *view, struct MacRect *output);
int64_t rinzu_macos_font_create(void *session, const uint8_t *name, size_t length, double size,
                                _Bool bold);
void *rinzu_macos_font(int64_t id);
int32_t rinzu_macos_font_destroy(void *font);
int64_t rinzu_macos_text_layout(void *font, const uint8_t *text, size_t length, double width,
                                int64_t alignment);
void *rinzu_macos_text(int64_t id);
int32_t rinzu_macos_text_metrics(void *text, struct MacTextMetrics *output);
int64_t rinzu_macos_text_texture(void *device, void *text, double scale);
int32_t rinzu_macos_text_destroy(void *text);
int64_t rinzu_macos_surface_scoped(void *device, void *window, const struct MacRect *rect);
int64_t rinzu_macos_frame_create(void *view, int64_t fps);
void *rinzu_macos_frame(int64_t id);
int32_t rinzu_macos_frame_request(void *clock);
int32_t rinzu_macos_frame_state(void *clock);
int32_t rinzu_macos_frame_animating(void *clock, _Bool enabled);
int32_t rinzu_macos_frame_retry(void *clock);
int32_t rinzu_macos_frame_presented(void *clock);
int32_t rinzu_macos_frame_statistics(void *clock, struct MacFrameStatistics *output);
int32_t rinzu_macos_frame_wait_begin(void *clock, int64_t token);
_Bool rinzu_macos_frame_wait_owned(void *clock, int64_t token);
void *rinzu_macos_frame_completion(void *clock, int64_t token);
int32_t rinzu_macos_frame_take(void *clock, int64_t token, struct MacFrameInfo *output);
int32_t rinzu_macos_frame_cancel(void *clock, int64_t token);
int32_t rinzu_macos_frame_finish(void *clock, int64_t token);
int32_t rinzu_macos_frame_stop(void *clock);
int32_t rinzu_macos_frame_destroy(void *clock);
int32_t rinzu_macos_frame_stop_window(void *window);
int64_t rinzu_macos_library_create(void *device, const uint8_t *source, size_t length, _Bool file);
void *rinzu_macos_library(int64_t id);
int32_t rinzu_macos_library_destroy(void *library);
int64_t rinzu_macos_compute_pipeline(void *library, const uint8_t *name, size_t length);
void *rinzu_macos_compute(int64_t id);
int32_t rinzu_macos_compute_destroy(void *pipeline);
int64_t rinzu_macos_render_pipeline(void *library, const uint8_t *vertex, size_t vertexLength,
                                    const uint8_t *fragment, size_t fragmentLength);
int64_t rinzu_macos_render_pipeline_options(void *library, const uint8_t *vertex,
                                            size_t vertex_length, const uint8_t *fragment,
                                            size_t fragment_length, int64_t format, int64_t blend);
void *rinzu_macos_render(int64_t id);
int32_t rinzu_macos_render_destroy(void *pipeline);
int64_t rinzu_macos_surface_create(void *device, const struct MacRect *rect);

int64_t rinzu_macos_command_create(void *device);
void *rinzu_macos_command(int64_t id);
int32_t rinzu_macos_command_destroy(void *command);
int32_t rinzu_macos_command_compute(void *command, void *pipeline);
int32_t rinzu_macos_command_render(void *command, void *view, const struct MacColor *clear);
int32_t rinzu_macos_command_render_texture(void *command, void *texture,
                                           const struct MacColor *clear);
int32_t rinzu_macos_command_viewport(void *command, const struct MacRect *viewport);
int32_t rinzu_macos_command_scissor(void *command, const struct MacScissor *scissor);
int32_t rinzu_macos_command_texture(void *command, void *texture, int64_t index, int32_t stage);
int32_t rinzu_macos_command_sampler(void *command, void *sampler, int64_t index, int32_t stage);
int32_t rinzu_macos_command_pipeline(void *command, void *pipeline);
int32_t rinzu_macos_command_buffer(void *command, void *buffer, int64_t offset, int64_t index,
                                   int32_t stage);
int32_t rinzu_macos_command_dispatch(void *command, int64_t x, int64_t y, int64_t z, int64_t tx,
                                     int64_t ty, int64_t tz);
int32_t rinzu_macos_command_draw(void *command, int64_t start, int64_t count);
int32_t rinzu_macos_command_commit(void *command);
int32_t rinzu_macos_command_abandon(void *command);
int64_t rinzu_macos_wait_token(void);
int32_t rinzu_macos_command_wait_begin(void *command, int64_t token);
_Bool rinzu_macos_command_wait_owned(void *command, int64_t token);
int32_t rinzu_macos_command_state(void *command);
void *rinzu_macos_command_completion(void *command, int64_t token);
int32_t rinzu_macos_command_status(void *command);
int32_t rinzu_macos_command_finish(void *command, int64_t token);
void *rinzu_macos_pending_command(void *session);
_Bool rinzu_macos_has_pending_command(void *session);
#endif
