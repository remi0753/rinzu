#include "rinzu/resource_report.h"
#include <assert.h>
static void *worker(void *unused) {
  (void)unused;
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  for (unsigned index = 0; index < 10000; ++index) {
    uint64_t value = rinzu_managed_string(&data, (const unsigned char *)"worker", 6);
    rinzu_managed_publish(&roots, &value, 1);
    rinzu_managed_collect(&data, &roots);
    rinzu_managed_publish(&roots, NULL, 0);
    rinzu_managed_collect(&data, &roots);
  }
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  return NULL;
}
int main(int argc, char **argv) {
  assert(argc == 2);
  assert(!rinzu_resource_report_start(argv[1], 1));
  pthread_t workers[4];
  for (unsigned index = 0; index < 4; ++index)
    assert(!pthread_create(&workers[index], NULL, worker, NULL));
  for (unsigned index = 0; index < 4; ++index)
    assert(!pthread_join(workers[index], NULL));
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].allocated == 40000);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].released == 40000);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live == 0);
  assert(!rinzu_resource_report_finish());
  puts("resource sampler concurrency passed");
  return 0;
}
