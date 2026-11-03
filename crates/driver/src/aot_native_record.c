/* Private native record layout and result plans. */
/* SOURCE_NATIVE_RECORD_LAYOUTS */
#ifdef RINZU_TOOLING_RECORD_LAYOUTS
#include RINZU_TOOLING_RECORD_LAYOUTS
#endif
static const rinzu_native_record_layout_t *rinzu_native_record_layout(uint64_t index) {
  if (!index || index > rinzu_native_record_layout_count)
    rinzu_interop_fatal();
  return &rinzu_native_record_layouts[index - 1];
}
#ifndef RINZU_AOT_PREPARED_RECORD_RECEIVERS
/* No-record and tooling images have no generated prepared-result plan. Keep
   their native result entry defined and fully checked through the generic API. */
static __attribute__((unused)) int
rinzu_native_receive_record_prepared(rinzu_managed_data_t *data,
                                     const rinzu_managed_root_set_t *root, uint64_t index,
                                     const unsigned char *input, uint64_t *output) {
  (void)root;
  return rinzu_managed_receive_record(data, rinzu_native_record_layout(index), input, output);
}
#endif
