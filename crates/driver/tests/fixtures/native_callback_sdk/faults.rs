fn replace(source: String, needle: &str, replacement: &str) -> String {
    assert_eq!(source.matches(needle).count(), 1, "{needle}");
    source.replace(needle, replacement)
}

pub fn source() -> String {
    let prelude = r#"
#include <pthread.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>
static _Thread_local void *rinzu_fault_outer,*rinzu_fault_previous;
static _Thread_local ProbeCallback rinzu_fault_callback;
static pthread_mutex_t rinzu_fault_mutex=PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t rinzu_fault_changed=PTHREAD_COND_INITIALIZER;
static void *rinzu_fault_owner_tokens[2];
static size_t rinzu_fault_owners;
static int rinzu_fault_is(const char *name) {
  const char *fault=getenv("RINZU_TEST_CALLBACK_FAULT");return fault && !strcmp(fault,name);
}
static void *rinzu_fault_thread(void *raw) {
  ProbeCallback callback=*(ProbeCallback *)raw;
  (void)callback(NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,0,0,0,0,0);_Exit(90);
}
static void *rinzu_fault_other_owner(void *token) {
  assert(!pthread_mutex_lock(&rinzu_fault_mutex));
  size_t own=rinzu_fault_owners++;assert(own<2);rinzu_fault_owner_tokens[own]=token;
  assert(!pthread_cond_broadcast(&rinzu_fault_changed));
  while (rinzu_fault_owners<2) assert(!pthread_cond_wait(&rinzu_fault_changed,&rinzu_fault_mutex));
  void *other=rinzu_fault_owner_tokens[1-own];assert(!pthread_mutex_unlock(&rinzu_fault_mutex));return other;
}
"#;
    let source = replace(
        include_str!("api.c").into(),
        "static unsigned char handle;",
        &format!("static unsigned char handle;\n{prelude}"),
    );
    let source = replace(
        source,
        "assert(data && callback);\n  uint64_t result = 0;",
        "assert(data && callback);if(rinzu_fault_is(\"scope\"))data=rinzu_fault_outer;uint64_t result=0;",
    );
    let source = replace(
        source,
        "void cb_body(void) {\n  bodies++;\n}",
        "void cb_body(void) {bodies++;if(getenv(\"RINZU_TEST_CALLBACK_FAULT\"))assert(write(2,\"CALLBACK_BODY_ENTER\\n\",20)==20);}",
    );
    let source = replace(
        source,
        "void *tokens[5] = {first, same, second, event, other};",
        r#"
    if(getenv("RINZU_TEST_CALLBACK_FAULT"))assert(write(2,"CALLBACK_NATIVE_ENTER\n",22)==22);
    rinzu_fault_outer=first;
    void *tokens[5]={first,same,second,event,other};
    "#,
    );
    let source = replace(
        source,
        "uint64_t before = touches, body_before = bodies, result = 0;",
        r#"
    const unsigned char *input_bytes=bytes;
    if(rinzu_fault_is("type"))second=other;
    if(rinzu_fault_is("unknown"))second=(void *)(uintptr_t)1;
    if(rinzu_fault_is("scope-closed")) {void *prior=rinzu_fault_previous;rinzu_fault_previous=second;if(prior)second=prior;}
    if(rinzu_fault_is("callback-closed")) {ProbeCallback prior=rinzu_fault_callback;rinzu_fault_callback=callback;if(prior)callback=prior;}
    if(rinzu_fault_is("owner"))second=rinzu_fault_other_owner(second);
    if(rinzu_fault_is("thread")) {pthread_t thread;assert(!pthread_create(&thread,NULL,rinzu_fault_thread,&callback));assert(!pthread_join(thread,NULL));_Exit(90);}
    if(rinzu_fault_is("bytes-null"))input_bytes=NULL;
    if(rinzu_fault_is("bytes-unreadable")) {void *page=mmap(NULL,16384,PROT_NONE,MAP_ANON|MAP_PRIVATE,-1,0);assert(page!=MAP_FAILED);input_bytes=page;}
    uint64_t before=touches,body_before=bodies,result=0;
    "#,
    );
    replace(
        source,
        "same, first, event, other, bytes,",
        "same, first, event, other, input_bytes,",
    )
}
