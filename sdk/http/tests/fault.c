#include <curl/curl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <assert.h>
#include <time.h>
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gate_changed = PTHREAD_COND_INITIALIZER;
static bool blocked_mode, released;
static unsigned entered;
static CURLcode controlled_perform(CURL *easy) {
  pthread_mutex_lock(&gate);
  if (blocked_mode) {
    entered++;
    pthread_cond_broadcast(&gate_changed);
    while (!released)
      pthread_cond_wait(&gate_changed, &gate);
    pthread_mutex_unlock(&gate);
    return CURLE_OK;
  }
  pthread_mutex_unlock(&gate);
  return curl_easy_perform(easy);
}
#define curl_easy_perform controlled_perform
#include "../native/src/http.c"
#undef curl_easy_perform
static void wait_done(struct Client *c) {
  while (!rinzu_native_completion_is_done(&c->completion.base)) {
    struct timespec t = {.tv_nsec = 1000000};
    nanosleep(&t, NULL);
  }
}
static void close_client(struct Client *c) {
  assert(rinzu_http_close(c) == 0);
  wait_done(c);
  assert(rinzu_http_status(c) == 0);
  rinzu_http_finish(c);
}
static void *wrong_owner(void *value) {
  assert(!rinzu_http_owner_has_client());
  assert(rinzu_http_ready(value) == RINZU_HTTP_OWNER);
  assert(rinzu_http_request(value, "http://127.0.0.1/", NULL, 0, false, 1000) == RINZU_HTTP_OWNER);
  return NULL;
}
int main(int argc, char **argv) {
  assert(argc == 4);
  struct Client *c = rinzu_http_new();
  assert(rinzu_http_owner_has_client() && rinzu_http_owner_client() == c);
  assert(rinzu_http_ready(c) == 0 && !rinzu_http_busy(c));
  pthread_t wrong;
  assert(!pthread_create(&wrong, NULL, wrong_owner, c));
  assert(!pthread_join(wrong, NULL));
  c->easy = curl_easy_init();
  assert(c->easy);
  assert(curl_easy_setopt(c->easy, CURLOPT_CAINFO, argv[3]) == CURLE_OK);
  assert(rinzu_http_request(c, argv[1], NULL, 0, false, 2000) == 0);
  assert(rinzu_http_busy(c) && rinzu_http_ready(c) == RINZU_HTTP_BUSY);
  wait_done(c);
  assert(rinzu_http_status(c) == 0);
  assert(rinzu_http_bytes(c)->length == 8192);
  rinzu_http_finish(c);
  assert(curl_easy_setopt(c->easy, CURLOPT_CAINFO, argv[3]) == CURLE_OK);
  assert(rinzu_http_request(c, argv[2], NULL, 0, false, 2000) == 0);
  wait_done(c);
  assert(rinzu_http_status(c) == CURLE_PEER_FAILED_VERIFICATION);
  rinzu_http_finish(c);
  close_client(c);
  assert(!rinzu_http_owner_has_client() && !rinzu_http_busy(c));
  assert(rinzu_http_ready(c) == RINZU_HTTP_CLOSED);
  pthread_mutex_lock(&gate);
  blocked_mode = true;
  pthread_mutex_unlock(&gate);
  struct Client *clients[35];
  uint8_t original[] = {0, 255, 2};
  for (unsigned i = 0; i < 35; i++)
    clients[i] = rinzu_http_new();
  for (unsigned i = 0; i < 2; i++)
    assert(!rinzu_http_request(clients[i], "http://127.0.0.1/", original, sizeof(original), true,
                               2000));
  pthread_mutex_lock(&gate);
  while (entered != 2)
    pthread_cond_wait(&gate_changed, &gate);
  pthread_mutex_unlock(&gate);
  original[1] = 1;
  assert(clients[0]->body[1] == 255);
  original[1] = 255;
  for (unsigned i = 2; i < 34; i++)
    assert(!rinzu_http_request(clients[i], "http://127.0.0.1/", original, sizeof(original), true,
                               2000));
  assert(rinzu_http_request(clients[34], "http://127.0.0.1/", original, sizeof(original), true,
                            2000) == RINZU_HTTP_SATURATED);
  assert(rinzu_http_live_buffers() == 34);
  rinzu_http_cancel(clients[0]);
  assert(rinzu_http_close(clients[0]) == RINZU_HTTP_BUSY);
  assert(!rinzu_native_completion_is_done(&clients[0]->completion.base));
  for (unsigned i = 1; i < 34; i++)
    rinzu_http_cancel(clients[i]);
  pthread_mutex_lock(&gate);
  released = true;
  pthread_cond_broadcast(&gate_changed);
  pthread_mutex_unlock(&gate);
  for (unsigned i = 0; i < 34; i++) {
    wait_done(clients[i]);
    rinzu_http_finish(clients[i]);
  }
  for (unsigned i = 0; i < 35; i++) {
    close_client(clients[i]);
    assert(rinzu_http_close(clients[i]) == RINZU_HTTP_CLOSED);
  }
  assert(rinzu_http_live_clients() == 0 && rinzu_http_live_buffers() == 0 && operations == 0 &&
         queued == 0 && !running);
  assert(!rinzu_http_owner_has_client());
  puts("http-fault-ok");
  return 0;
}
