struct SavedXattr {
  struct SavedXattr *next;
  char *name;
  void *data;
  size_t length;
};

struct Metadata {
  struct SavedXattr *xattrs;
  acl_t acl;
#ifdef __APPLE__
  bool has_birthtime;
  struct timespec birthtime;
#endif
};

static void free_metadata(struct Metadata *metadata) {
  if (metadata->acl)
    acl_free(metadata->acl);
  while (metadata->xattrs) {
    struct SavedXattr *attribute = metadata->xattrs;
    metadata->xattrs = attribute->next;
    free(attribute->name);
    free(attribute->data);
    free(attribute);
  }
}
