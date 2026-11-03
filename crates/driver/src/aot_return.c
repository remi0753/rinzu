/* A return value stays rooted through finally's stack-map replacement. */
typedef struct RinzuReturnRoot {
  uint64_t depth;
  rinzu_managed_roots_t roots;
  rinzu_managed_root_set_t attached;
  struct RinzuReturnRoot *next;
} RinzuReturnRoot;
void rinzu_baseline_retain_return(void *raw, uint64_t value) {
  RinzuContext *context = raw;
  RinzuReturnRoot *root = context->return_roots;
  if (!context->roots.depth || (root && root->depth == context->roots.depth))
    rinzu_failure_fatal();
  root = rinzu_failure_allocate(sizeof(*root));
  root->depth = context->roots.depth;
  root->next = context->return_roots;
  rinzu_managed_enter(&root->roots);
  rinzu_managed_publish(&root->roots, &value, 1);
  rinzu_managed_attach_roots(rinzu_context_data(context), &root->attached, &root->roots);
  context->return_roots = root;
}
static void rinzu_baseline_release_return(RinzuContext *context) {
  RinzuReturnRoot *root = context->return_roots;
  if (!root || root->depth != context->roots.depth)
    return;
  context->return_roots = root->next;
  rinzu_managed_leave(&root->roots);
  rinzu_managed_detach_roots(rinzu_context_data(context), &root->attached);
  rinzu_managed_dispose(&root->roots);
  free(root);
}
