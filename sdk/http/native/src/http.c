#include "rinzu_http.h"
#include "rinzu/native_completion.h"
#include <curl/curl.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <errno.h>
struct Client {
  rinzu_native_completion_v2_t completion;
  pthread_t owner;
  bool owns, busy, ready, closing, post, overflow;
  CURL *easy;
  char *url, *method;
  struct curl_slist *headers;
  uint8_t *body;
  rinzu_native_byte_array_t response, response_headers;
  size_t length;
  long timeout, http;
  int64_t sent;
  int32_t status;
  struct Client *next_owner;
};
enum { THREADS = 2, QUEUE = 32, HEADER_BYTES = 32768, HEADER_COUNT = 64, TOKEN_BYTES = 128 };
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static pthread_once_t initialized = PTHREAD_ONCE_INIT;
static pthread_t workers[THREADS];
static struct Client *queue[QUEUE];
static size_t head, queued, operations, owners, buffers, retained;
static bool running, stopping;
static int32_t backend_status;
static _Thread_local struct Client *owner_clients;
static void fatal(void) {
  _Exit(86);
}
static void *allocate(size_t size) {
  void *value = calloc(1, size ? size : 1);
  if (!value)
    _Exit(70);
  return value;
}
static void initialize(void) {
  CURLcode status = curl_global_init(CURL_GLOBAL_DEFAULT);
  if (status == CURLE_OUT_OF_MEMORY)
    _Exit(70);
  if (status) {
    backend_status = RINZU_HTTP_BACKEND;
    return;
  }
  const curl_version_info_data *v = curl_version_info(CURLVERSION_NOW);
  if (!v || v->version_num < 0x080701 || !(v->features & CURL_VERSION_ASYNCHDNS) ||
      !(v->features & CURL_VERSION_THREADSAFE))
    backend_status = RINZU_HTTP_BACKEND;
  /* System global initialization is process-held; handles/payload are explicit. */
}
static int32_t check(struct Client *c) {
  if (!c)
    fatal();
  return pthread_equal(c->owner, pthread_self()) ? 0 : RINZU_HTTP_OWNER;
}
static bool cancelled(struct Client *c) {
  return atomic_load_explicit(&c->completion.base.cancelled, memory_order_acquire) != 0;
}
static int progress(void *context, curl_off_t total, curl_off_t now, curl_off_t up_total,
                    curl_off_t up_now) {
  (void)total;
  (void)now;
  (void)up_total;
  (void)up_now;
  return cancelled(context) ? 1 : 0;
}
static size_t receive(char *data, size_t size, size_t count, void *context) {
  struct Client *c = context;
  if ((size && count > SIZE_MAX / size) || size * count > RINZU_HTTP_CHUNK - c->response.length) {
    c->overflow = true;
    return 0;
  }
  size_t length = size * count;
  memcpy(c->response.data + c->response.length, data, length);
  c->response.length += length;
  return length;
}

static bool token(const uint8_t *data, size_t length) {
  if (!length)
    return false;
  for (size_t i = 0; i < length; ++i) {
    uint8_t byte = data[i];
    if (!((byte >= 'a' && byte <= 'z') || (byte >= 'A' && byte <= 'Z') ||
          (byte >= '0' && byte <= '9') || (byte && strchr("!#$%&'*+-.^_`|~", byte))))
      return false;
  }
  return true;
}

static bool field_is(const uint8_t *name, size_t length, const char *word) {
  return length == strlen(word) && !strncasecmp((const char *)name, word, length);
}

/* The Source API supplies validated name/value pairs as colon-delimited lines.
   Keep HTTP framing under libcurl's control and take ownership before queuing. */
static int32_t request_headers(const uint8_t *data, size_t length, struct curl_slist **fields) {
  if (length > HEADER_BYTES || (length && !data))
    return RINZU_HTTP_ARGUMENT;
  size_t count = 0;
  for (size_t offset = 0; offset < length;) {
    const uint8_t *line = data + offset;
    const uint8_t *lf = memchr(line, '\n', length - offset);
    if (++count > HEADER_COUNT || !lf || lf == line || lf[-1] != '\r')
      return RINZU_HTTP_ARGUMENT;
    const uint8_t *end = lf - 1;
    const uint8_t *colon = memchr(line, ':', (size_t)(end - line));
    if (!colon)
      return RINZU_HTTP_ARGUMENT;
    size_t name = (size_t)(colon - line);
    size_t value = (size_t)(end - colon - 1);
    if (name > TOKEN_BYTES || !token(line, name) || field_is(line, name, "content-length") ||
        field_is(line, name, "transfer-encoding") || field_is(line, name, "trailer") ||
        field_is(line, name, "upgrade"))
      return RINZU_HTTP_ARGUMENT;
    for (const uint8_t *p = colon + 1; p < end; ++p)
      if ((*p < 32 && *p != '\t') || *p == 127)
        return RINZU_HTTP_ARGUMENT;
    char field[HEADER_BYTES + 1];
    memcpy(field, line, name);
    if (value) {
      field[name] = ':';
      field[name + 1] = ' ';
      memcpy(field + name + 2, colon + 1, value);
      field[name + 2 + value] = 0;
    } else {
      /* A literal empty value uses libcurl's semicolon form. */
      field[name] = ';';
      field[name + 1] = 0;
    }
    struct curl_slist *next = curl_slist_append(*fields, field);
    if (!next)
      _Exit(70);
    *fields = next;
    offset = (size_t)(lf - data) + 1;
  }
  return 0;
}

static int32_t collect_headers(struct Client *c) {
  size_t capacity = 0, count = 0;
  struct curl_header *field = NULL;
  rinzu_native_byte_array_t *out = &c->response_headers;
  while ((field = curl_easy_nextheader(c->easy, CURLH_HEADER, -1, field))) {
    size_t name = strlen(field->name), value = strlen(field->value);
    if (++count > HEADER_COUNT || name + value + 2 > HEADER_BYTES - out->length)
      return RINZU_HTTP_BOUND;
    if (!token((const uint8_t *)field->name, name))
      return CURLE_WEIRD_SERVER_REPLY;
    size_t needed = out->length + name + value + 2;
    if (needed > capacity) {
      capacity = capacity && capacity * 2 > needed ? capacity * 2 : needed;
      if (capacity > HEADER_BYTES)
        capacity = HEADER_BYTES;
      uint8_t *next = realloc(out->data, capacity);
      if (!next)
        _Exit(70);
      out->data = next;
    }
    uint8_t *data = out->data + out->length;
    for (size_t i = 0; i < name; ++i) {
      uint8_t byte = (uint8_t)field->name[i];
      data[i] = byte >= 'A' && byte <= 'Z' ? (uint8_t)(byte + 'a' - 'A') : byte;
    }
    data[name] = 0;
    memcpy(data + name + 1, field->value, value + 1);
    out->length = needed;
  }
  return 0;
}
static CURLUcode url_part(CURLU *url, CURLUPart part, char **value) {
  CURLUcode result = curl_url_get(url, part, value, 0);
  if (result == CURLUE_OUT_OF_MEMORY)
    _Exit(70);
  return result;
}
static int32_t validate_url(const char *url) {
  size_t length = strlen(url);
  if (!length || length > 4096)
    return RINZU_HTTP_ARGUMENT;
  CURLU *u = curl_url();
  if (!u)
    _Exit(70);
  char *scheme = NULL, *host = NULL, *user = NULL, *password = NULL;
  CURLUcode result = curl_url_set(u, CURLUPART_URL, url, 0);
  if (result == CURLUE_OUT_OF_MEMORY)
    _Exit(70);
  bool valid =
      !result && !url_part(u, CURLUPART_SCHEME, &scheme) && !url_part(u, CURLUPART_HOST, &host);
  if (valid) {
    bool https = !strcmp(scheme, "https");
    bool local = !strcmp(host, "127.0.0.1") || !strcmp(host, "localhost") || !strcmp(host, "[::1]");
    valid = https || (!strcmp(scheme, "http") && local);
    if (url_part(u, CURLUPART_USER, &user) == CURLUE_OK ||
        url_part(u, CURLUPART_PASSWORD, &password) == CURLUE_OK)
      valid = false;
  }
  curl_free(scheme);
  curl_free(host);
  curl_free(user);
  curl_free(password);
  curl_url_cleanup(u);
  return valid ? 0 : RINZU_HTTP_ARGUMENT;
}
static void perform(struct Client *c) {
  int32_t status = 0;
  if (c->closing) {
    if (c->easy) {
      curl_easy_cleanup(c->easy);
      c->easy = NULL;
    }
  } else if (cancelled(c))
    status = CURLE_ABORTED_BY_CALLBACK;
  else {
    if (!c->easy) {
      c->easy = curl_easy_init();
      if (!c->easy)
        _Exit(70);
    }
    CURLcode configured = CURLE_OK;
#define SET(option, value)                                                                         \
  do {                                                                                             \
    if (configured == CURLE_OK)                                                                    \
      configured = curl_easy_setopt(c->easy, option, value);                                       \
  } while (0)
    SET(CURLOPT_URL, c->url);
    SET(CURLOPT_PROTOCOLS_STR, "http,https");
    SET(CURLOPT_PROXY, "");
    SET(CURLOPT_NOSIGNAL, 1L);
    SET(CURLOPT_TIMEOUT_MS, c->timeout);
    SET(CURLOPT_CONNECTTIMEOUT_MS, c->timeout);
    SET(CURLOPT_FOLLOWLOCATION, 0L);
    SET(CURLOPT_SSL_VERIFYPEER, 1L);
    SET(CURLOPT_SSL_VERIFYHOST, 2L);
    SET(CURLOPT_WRITEFUNCTION, receive);
    SET(CURLOPT_WRITEDATA, c);
    SET(CURLOPT_NOPROGRESS, 0L);
    SET(CURLOPT_XFERINFOFUNCTION, progress);
    SET(CURLOPT_XFERINFODATA, c);
    if (c->method) {
      SET(CURLOPT_CUSTOMREQUEST, c->method);
      SET(CURLOPT_HTTPHEADER, c->headers);
      if (!strcmp(c->method, "HEAD"))
        SET(CURLOPT_NOBODY, 1L);
    }
    if (c->post) {
      SET(CURLOPT_POST, 1L);
      SET(CURLOPT_POSTFIELDS, c->body);
      SET(CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)c->length);
    }
    status = (int32_t)(configured ? configured : curl_easy_perform(c->easy));
#undef SET
    curl_off_t sent = 0;
    CURLcode info = curl_easy_getinfo(c->easy, CURLINFO_RESPONSE_CODE, &c->http);
    if (!status && info)
      status = (int32_t)info;
    info = curl_easy_getinfo(c->easy, CURLINFO_SIZE_UPLOAD_T, &sent);
    if (!status && info)
      status = (int32_t)info;
    if (sent < 0 || sent > INT64_MAX)
      fatal();
    c->sent = (int64_t)sent;
    if (c->overflow)
      status = RINZU_HTTP_BOUND;
    if (!status && c->method)
      status = collect_headers(c);
    if (status == CURLE_OUT_OF_MEMORY)
      _Exit(70);
    /* Drop every option referring to operation payload before publication. */
    curl_easy_reset(c->easy);
  }
  pthread_mutex_lock(&lock);
  c->status = status;
  c->ready = true;
  rinzu_native_completion_wake_t *target =
      rinzu_native_completion_v2_publish_target(&c->completion);
  pthread_cond_broadcast(&changed);
  pthread_mutex_unlock(&lock);
  if (target) {
    target->notify(target);
    target->release(target);
  }
}
static void *worker(void *unused) {
  (void)unused;
  for (;;) {
    pthread_mutex_lock(&lock);
    while (!queued && !stopping)
      pthread_cond_wait(&changed, &lock);
    if (!queued && stopping) {
      pthread_mutex_unlock(&lock);
      return NULL;
    }
    struct Client *c = queue[head];
    head = (head + 1) % QUEUE;
    queued--;
    pthread_mutex_unlock(&lock);
    perform(c);
  }
}
static int32_t start_pool(void) {
  while (stopping)
    pthread_cond_wait(&changed, &lock);
  if (running)
    return 0;
  running = true;
  for (size_t i = 0; i < THREADS; i++) {
    int result = pthread_create(&workers[i], NULL, worker, NULL);
    if (!result)
      continue;
    if (result == ENOMEM)
      _Exit(70);
    stopping = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&lock);
    for (size_t n = 0; n < i; n++)
      if (pthread_join(workers[n], NULL))
        fatal();
    pthread_mutex_lock(&lock);
    running = stopping = false;
    pthread_cond_broadcast(&changed);
    return result;
  }
  return 0;
}
void *rinzu_http_new(void) {
  if (pthread_once(&initialized, initialize))
    fatal();
  struct Client *c = allocate(sizeof(*c));
  c->owner = pthread_self();
  c->owns = true;
  c->next_owner = owner_clients;
  owner_clients = c;
  rinzu_native_completion_v2_init(&c->completion);
  pthread_mutex_lock(&lock);
  owners++;
  retained += sizeof(*c);
  pthread_mutex_unlock(&lock);
  return c;
}
bool rinzu_http_owner_has_client(void) {
  pthread_mutex_lock(&lock);
  bool found = false;
  for (struct Client *c = owner_clients; c; c = c->next_owner)
    if (c->owns) {
      found = true;
      break;
    }
  pthread_mutex_unlock(&lock);
  return found;
}
void *rinzu_http_owner_client(void) {
  pthread_mutex_lock(&lock);
  struct Client *found = NULL;
  for (struct Client *c = owner_clients; c; c = c->next_owner)
    if (c->owns) {
      found = c;
      break;
    }
  pthread_mutex_unlock(&lock);
  if (!found)
    fatal();
  return found;
}
bool rinzu_http_busy(void *value) {
  struct Client *c = value;
  if (check(c))
    fatal();
  pthread_mutex_lock(&lock);
  bool result = c->busy;
  pthread_mutex_unlock(&lock);
  return result;
}
int32_t rinzu_http_ready(void *value) {
  struct Client *c = value;
  int status = check(c);
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  status = !c->owns ? RINZU_HTTP_CLOSED : c->busy ? RINZU_HTTP_BUSY : 0;
  pthread_mutex_unlock(&lock);
  return status;
}
static int32_t admission(struct Client *c) {
  if (!c->owns)
    return RINZU_HTTP_CLOSED;
  if (c->busy)
    return RINZU_HTTP_BUSY;
  if (queued == QUEUE)
    return RINZU_HTTP_SATURATED;
  if (backend_status)
    return backend_status;
  return start_pool();
}
static void submit(struct Client *c, bool closing) {
  c->closing = closing;
  c->busy = true;
  c->ready = false;
  c->overflow = false;
  c->response.length = 0;
  c->sent = 0;
  c->http = 0;
  c->status = 0;
  rinzu_native_completion_v2_init(&c->completion);
  operations++;
  queue[(head + queued) % QUEUE] = c;
  queued++;
  pthread_cond_signal(&changed);
}
static int32_t request(void *value, const char *method, const char *url,
                       const uint8_t *header_bytes, size_t header_length, const uint8_t *body,
                       size_t length, bool post, int64_t timeout) {
  struct Client *c = value;
  int32_t status = check(c);
  if (status)
    return status;
  if (timeout < 1 || timeout > 30000 || length > RINZU_HTTP_CHUNK || (!post && length) ||
      (!body && length))
    return RINZU_HTTP_ARGUMENT;
  status = validate_url(url);
  if (status)
    return status;
  struct curl_slist *fields = NULL;
  status = request_headers(header_bytes, header_length, &fields);
  if (status) {
    curl_slist_free_all(fields);
    return status;
  }
  pthread_mutex_lock(&lock);
  status = admission(c);
  if (!status) {
    c->url = allocate(strlen(url) + 1);
    strcpy(c->url, url);
    if (method) {
      c->method = allocate(strlen(method) + 1);
      strcpy(c->method, method);
    }
    c->headers = fields;
    c->body = allocate(length);
    if (length)
      memcpy(c->body, body, length);
    c->response.data = allocate(RINZU_HTTP_CHUNK);
    buffers++;
    c->length = length;
    c->post = post;
    c->timeout = (long)timeout;
    submit(c, false);
  }
  pthread_mutex_unlock(&lock);
  if (status)
    curl_slist_free_all(fields);
  return status;
}
int32_t rinzu_http_request(void *value, const char *url, const uint8_t *body, size_t length,
                           bool post, int64_t timeout) {
  return request(value, NULL, url, NULL, 0, body, length, post, timeout);
}
int32_t rinzu_http_send(void *value, const char *method, const char *url, const uint8_t *headers,
                        size_t header_length, const uint8_t *body, size_t length, int64_t timeout) {
  size_t method_length = strlen(method);
  if (method_length > TOKEN_BYTES || !token((const uint8_t *)method, method_length) ||
      (!strcmp(method, "HEAD") && length))
    return RINZU_HTTP_ARGUMENT;
  return request(value, method, url, headers, header_length, body, length,
                 length || !strcmp(method, "POST"), timeout);
}
int32_t rinzu_http_close(void *value) {
  struct Client *c = value;
  int32_t status = check(c);
  if (status)
    return status;
  pthread_mutex_lock(&lock);
  if (!c->owns)
    status = RINZU_HTTP_CLOSED;
  else if (c->busy)
    status = RINZU_HTTP_BUSY;
  else if (queued == QUEUE)
    status = RINZU_HTTP_SATURATED;
  else
    status = start_pool();
  if (!status)
    submit(c, true);
  pthread_mutex_unlock(&lock);
  return status;
}
void *rinzu_http_completion(void *value) {
  struct Client *c = value;
  if (check(c) || !c->busy)
    fatal();
  return &c->completion.base;
}
static struct Client *completed(void *value) {
  struct Client *c = value;
  if (check(c))
    fatal();
  pthread_mutex_lock(&lock);
  bool valid = c->busy && c->ready;
  pthread_mutex_unlock(&lock);
  if (!valid)
    fatal();
  return c;
}
int32_t rinzu_http_status(void *value) {
  return completed(value)->status;
}
int64_t rinzu_http_response_status(void *value) {
  return (int64_t)completed(value)->http;
}
int64_t rinzu_http_sent(void *value) {
  return completed(value)->sent;
}
const rinzu_native_byte_array_t *rinzu_http_bytes(void *value) {
  return &completed(value)->response;
}
const rinzu_native_byte_array_t *rinzu_http_headers(void *value) {
  return &completed(value)->response_headers;
}
void rinzu_http_cancel(void *value) {
  struct Client *c = value;
  if (check(c))
    fatal();
  atomic_store_explicit(&c->completion.base.cancelled, 1, memory_order_release);
}
void rinzu_http_finish(void *value) {
  struct Client *c = completed(value);
  pthread_mutex_lock(&lock);
  if (c->response.data) {
    free(c->response.data);
    c->response = (rinzu_native_byte_array_t){0};
    buffers--;
  }
  free(c->response_headers.data);
  c->response_headers = (rinzu_native_byte_array_t){0};
  curl_slist_free_all(c->headers);
  c->headers = NULL;
  free(c->method);
  c->method = NULL;
  free(c->body);
  c->body = NULL;
  free(c->url);
  c->url = NULL;
  c->busy = false;
  c->ready = false;
  operations--;
  if (c->closing) {
    c->owns = false;
    owners--;
  }
  bool stop = !owners && !operations && running;
  if (stop) {
    stopping = true;
    pthread_cond_broadcast(&changed);
  }
  pthread_mutex_unlock(&lock);
  if (stop) {
    for (size_t i = 0; i < THREADS; i++)
      if (pthread_join(workers[i], NULL))
        fatal();
    pthread_mutex_lock(&lock);
    running = stopping = false;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&lock);
  }
}
int64_t rinzu_http_live_clients(void) {
  pthread_mutex_lock(&lock);
  size_t n = owners;
  pthread_mutex_unlock(&lock);
  return (int64_t)n;
}
int64_t rinzu_http_live_buffers(void) {
  pthread_mutex_lock(&lock);
  size_t n = buffers;
  pthread_mutex_unlock(&lock);
  return (int64_t)n;
}
int64_t rinzu_http_retained_controls(void) {
  pthread_mutex_lock(&lock);
  size_t n = retained;
  pthread_mutex_unlock(&lock);
  return (int64_t)n;
}
