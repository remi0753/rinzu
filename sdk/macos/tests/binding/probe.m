// Appended to a private copy of macos.m; exercises real AppKit text input.
int32_t rinzu_macos_test_input(void *raw, int32_t mode) {
  RZMacView *view = (RZMacView *)lookup(raw, RZ_VIEW);
  if (!view || view.widgetKind != RZ_TEXT_VIEW)
    return RZ_INVALID;
  NSTextView *text = (NSTextView *)view.view;
  if (mode == 0) {
    [text insertText:@"入力 日本🙂" replacementRange:NSMakeRange(0, text.string.length)];
    [text setSelectedRange:NSMakeRange(3, 2)];
    return 0;
  }
  if (mode == 1) {
    [text setMarkedText:@"仮" selectedRange:NSMakeRange(1, 0) replacementRange:NSMakeRange(0, 0)];
    return text.hasMarkedText ? 1 : 0;
  }
  if (mode == 3) {
    [text setSelectedRange:NSMakeRange(0, 1)];
    return 0;
  }
  [text unmarkText];
  return 0;
}
