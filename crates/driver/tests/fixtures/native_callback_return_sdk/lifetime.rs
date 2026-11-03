pub fn retain_until_shutdown(root: &std::path::Path) {
    let path = root.join("returns/api.c");
    let source = std::fs::read_to_string(&path).unwrap();
    let old = "int32_t return_held(void) {\n  assert(!pthread_mutex_lock(&mutex));\n  Held *values = held;\n  held = NULL;\n  assert(!pthread_mutex_unlock(&mutex));\n  while (values) {\n    Held *next = values->next;\n    assert(!memcmp(values->data, values->expected, values->length));\n    free(values);\n    values = next;\n  }\n  return 1;\n}";
    assert_eq!(source.matches(old).count(), 1);
    let source = source.replace(old, r#"
static size_t check_held(void) {
  /* Bounded fixture pressure, never an attempt to exhaust OS memory. */
  unsigned char *pressure[16];
  for(size_t i=0;i<16;i++){pressure[i]=malloc(65536);if(!pressure[i])_Exit(71);memset(pressure[i],(int)i,65536);}
  size_t count=0;assert(!pthread_mutex_lock(&mutex));
  for(Held *node=held;node;node=node->next){assert(!memcmp(node->data,node->expected,node->length));count++;}
  assert(!pthread_mutex_unlock(&mutex));
  for(size_t i=0;i<16;i++)free(pressure[i]);return count;
}
int32_t return_held(void) {assert(check_held()>=56);return 1;}
static void check_after_shutdown(void) {
  size_t count=check_held();assert(count>=56);
  char message[80];int length=snprintf(message,sizeof(message),"RETURN_BUFFERS_AFTER_SHUTDOWN=%zu\n",count);
  assert(length>0 && (size_t)length<sizeof(message) && write(2,message,(size_t)length)==length);
  Held *values=held;held=NULL;while(values){Held *next=values->next;free(values);values=next;}
}
static void register_shutdown_check(void) {assert(!atexit(check_after_shutdown));}
"#);
    let marker = "  const unsigned char normal[] = \"";
    assert_eq!(source.matches(marker).count(), 1);
    let source = source.replace(marker, "  assert(!pthread_once(&shutdown_once,register_shutdown_check));\n  const unsigned char normal[]=\"");
    // Declarations precede the first physical native call; checks run at atexit.
    let source = source.replace("static _Thread_local size_t ticks;", "#include <stdio.h>\nstatic void register_shutdown_check(void);\nstatic pthread_once_t shutdown_once=PTHREAD_ONCE_INIT;\nstatic _Thread_local size_t ticks;");
    std::fs::write(path, source).unwrap();
}
