#!/usr/bin/env python3
"""Exercise the actual AOT ready-turn implementation with deterministic tasks."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
PRELUDE = r'''
#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <time.h>
typedef struct Future { uint64_t identity; int settled; } rinzu_async_future_t;
typedef struct Task {
  struct { int running, started; } continuation;
  rinzu_async_future_t *future, *waiting;
  struct Task *next;
} RinzuAsyncTask;
typedef struct { int unused; } Runtime;
typedef struct { Runtime runtime; RinzuAsyncTask *tasks; } RinzuAsyncState;
typedef struct { RinzuAsyncState *state; } RinzuContext;
static RinzuAsyncTask tasks[4098];
static rinzu_async_future_t futures[4098];
static uint64_t visits[4098];
static size_t allocations, releases, visited, total;
static int mode, verify;
static void rinzu_failure_fatal(void) { abort(); }
static void *rinzu_failure_allocate(size_t n) {
  allocations++; void *p = calloc(1, n); assert(p); return p;
}
static void release(void *p) { releases++; free(p); }
#define free release
static RinzuAsyncState *rinzu_aot_async_state(RinzuContext *c) { return c->state; }
static uint64_t rinzu_async_handle(rinzu_async_future_t *f) { return f->identity; }
static rinzu_async_future_t *rinzu_async_lookup(Runtime *r, uint64_t id) {
  (void)r; assert(id && id <= total); return &futures[id];
}
static RinzuAsyncTask *rinzu_aot_async_task(RinzuAsyncState *s, rinzu_async_future_t *f) {
  (void)s; return &tasks[f->identity];
}
static void rinzu_aot_async_resume_ready(RinzuContext *c);
static void append(RinzuAsyncState *s) {
  total++; futures[total] = (rinzu_async_future_t){.identity = total};
  tasks[total] = (RinzuAsyncTask){.future = &futures[total], .next = s->tasks};
  s->tasks = &tasks[total];
}
static void rinzu_aot_async_resume(RinzuContext *c, RinzuAsyncTask *t) {
  t->continuation.running = t->continuation.started = 1;
  if (verify) visits[visited++] = t->future->identity;
  if (t->future->identity == 1) {
    if (mode == 1) { tasks[2].continuation.started = 1; append(c->state); }
    if (mode == 2) rinzu_aot_async_resume_ready(c);
  }
  t->continuation.running = 0;
}
'''
POSTLUDE = r'''
static void setup(RinzuAsyncState *s, size_t count) {
  *s = (RinzuAsyncState){0}; total = visited = allocations = releases = 0;
  for (size_t i = 0; i < count; ++i) append(s);
}
static uint64_t tick(void) {
  struct timespec t; assert(!clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &t));
  return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}
int main(int argc, char **argv) {
  (void)argv;
  RinzuAsyncState s; RinzuContext c = {&s};
  verify = 1;
  const size_t sizes[] = {0, 1, 16, 64, 65, 257, 4096};
  for (size_t i = 0; i < sizeof sizes / sizeof *sizes; ++i) {
    size_t n = sizes[i]; setup(&s, n); rinzu_aot_async_resume_ready(&c);
    assert(visited == n && allocations == releases);
    assert(allocations == (n > 64));
    for (size_t j = 0; j < n; ++j) assert(visits[j] == j + 1);
  }
  mode = 1; setup(&s, 64); rinzu_aot_async_resume_ready(&c);
  assert(visited == 64 && visits[0] == 1 && visits[63] == 65);
  for (size_t i = 1; i < 63; ++i) assert(visits[i] == i + 2);
  assert(!allocations && !releases);
  mode = 2; setup(&s, 65); rinzu_aot_async_resume_ready(&c);
  assert(visited == 65 && allocations == 1 && releases == 1);
  for (size_t i = 0; i < 65; ++i) assert(visits[i] == i + 1);
  if (argc == 1) { puts("ready snapshot boundaries/order/cancel/reentry: passed"); return 0; }
  mode = verify = 0;
  for (size_t i = 1; i < 6; ++i) {
    size_t n = sizes[i], rounds = 200000;
    setup(&s, n); size_t count = 0; uint64_t start = tick();
    for (size_t j = 0; j < rounds; ++j) {
      for (size_t k = 1; k <= n; ++k) tasks[k].continuation.started = 0;
      rinzu_aot_async_resume_ready(&c); count += n;
    }
    uint64_t cpu = tick() - start;
    printf("{\"case\":\"ready-%zu\",\"operations\":%zu,\"nanoseconds\":%llu,"
           "\"cpu_nanoseconds\":%llu,\"allocations\":%zu,\"allocated_bytes\":%zu,\"checksum\":%zu}\n",
           n, rounds, (unsigned long long)cpu, (unsigned long long)cpu,
           allocations, allocations * n * sizeof(uint64_t), count);
  }
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, default=ROOT / 'crates/driver/src/aot_source_async.c')
    p.add_argument('--workdir', type=Path, required=True)
    p.add_argument('--baseline', action='store_true', help='Allow heap allocation for small snapshots')
    args = p.parse_args()
    args.workdir.mkdir(parents=True, exist_ok=True)
    text = args.source.read_text()
    start = text.index('static int rinzu_aot_async_compare_handles(')
    end = text.index('\n}\n', text.index('static void rinzu_aot_async_resume_ready(', start)) + 3
    post = POSTLUDE
    if args.baseline:
        post = post.replace('assert(allocations == (n > 64));', 'assert(allocations == (n > 0));')
        post = post.replace('assert(!allocations && !releases);', 'assert(allocations == 2 && releases == 2);')
        post = post.replace('allocations == 1 && releases == 1', 'allocations == 2 && releases == 2')
    source = args.workdir / 'ready.c'
    source.write_text(PRELUDE + text[start:end] + post)
    runs = []
    for sanitized in [False, True]:
        binary = args.workdir / ('san' if sanitized else 'ready')
        command = ['clang', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', str(source), '-o', str(binary)]
        if sanitized:
            command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
        subprocess.run(command, check=True)
        run = subprocess.run([str(binary)], check=True, capture_output=True, text=True)
        runs.append(dict(sanitized=sanitized, stdout=run.stdout))
    (args.workdir / 'tests.json').write_text(json.dumps(dict(passed=True, cases=runs), indent=2) + '\n')
    print(json.dumps(runs))


if __name__ == '__main__':
    main()
