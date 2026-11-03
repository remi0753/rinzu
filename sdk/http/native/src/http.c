#include "rinzu_http.h"
#include "rinzu/native_completion.h"
#include <curl/curl.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
struct Client {
  rinzu_native_completion_t completion;
  pthread_t owner;
  bool owns, busy, ready, closing, post, overflow;
  CURL *easy;
  char *url;
  uint8_t *body, *response;
  size_t length, received;
  long timeout, http;
  int64_t sent;
  int32_t status;
  struct Client *next_owner;
};
enum { THREADS = 2, QUEUE = 32 };
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static pthread_once_t initialized = PTHREAD_ONCE_INIT;
static pthread_t workers[THREADS];
static struct Client *queue[QUEUE];
static size_t head, queued, operations, owners, buffers, retained;
static bool running, stopping;
static int32_t backend_status;
static _Thread_local struct Client *owner_clients;
static const uint8_t empty[RINZU_HTTP_CHUNK];
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
  return atomic_load_explicit(&c->completion.cancelled, memory_order_acquire) != 0;
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
  if ((size && count > SIZE_MAX / size) || size * count > RINZU_HTTP_CHUNK - c->received) {
    c->overflow = true;
    return 0;
  }
  size_t length = size * count;
  memcpy(c->response + c->received, data, length);
  c->received += length;
  return length;
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
    if (status == CURLE_OUT_OF_MEMORY)
      _Exit(70);
    /* Drop every option referring to operation payload before publication. */
    curl_easy_reset(c->easy);
  }
  pthread_mutex_lock(&lock);
  c->status = status;
  c->ready = true;
  atomic_store_explicit(&c->completion.done, 1, memory_order_release);
  pthread_cond_broadcast(&changed);
  pthread_mutex_unlock(&lock);
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
  rinzu_native_completion_init(&c->completion);
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
  c->received = 0;
  c->sent = 0;
  c->http = 0;
  c->status = 0;
  rinzu_native_completion_init(&c->completion);
  operations++;
  queue[(head + queued) % QUEUE] = c;
  queued++;
  pthread_cond_signal(&changed);
}
int32_t rinzu_http_request(void *value, const char *url, const uint8_t *body, size_t length,
                           bool post, int64_t timeout) {
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
  pthread_mutex_lock(&lock);
  status = admission(c);
  if (!status) {
    c->url = allocate(strlen(url) + 1);
    strcpy(c->url, url);
    c->body = allocate(length);
    if (length)
      memcpy(c->body, body, length);
    c->response = allocate(RINZU_HTTP_CHUNK);
    buffers++;
    c->length = length;
    c->post = post;
    c->timeout = (long)timeout;
    submit(c, false);
  }
  pthread_mutex_unlock(&lock);
  return status;
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
  return &c->completion;
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
int64_t rinzu_http_length(void *value) {
  return (int64_t)completed(value)->received;
}
int64_t rinzu_http_sent(void *value) {
  return completed(value)->sent;
}
const uint8_t *rinzu_http_bytes(void *value) {
  struct Client *c = completed(value);
  return c->response ? c->response : empty;
}
void rinzu_http_cancel(void *value) {
  struct Client *c = value;
  if (check(c))
    fatal();
  atomic_store_explicit(&c->completion.cancelled, 1, memory_order_release);
}
void rinzu_http_finish(void *value) {
  struct Client *c = completed(value);
  pthread_mutex_lock(&lock);
  if (c->response) {
    free(c->response);
    c->response = NULL;
    buffers--;
  }
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
