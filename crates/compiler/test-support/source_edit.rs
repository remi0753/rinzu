/// Fixture edits must actually apply: a formatter or source change must not
/// silently remove a fault injection, GC variant, or scheduling permutation.
pub trait ReplaceChecked {
    fn replace_checked(&self, needle: &str, replacement: &str) -> String;
}

impl ReplaceChecked for str {
    fn replace_checked(&self, needle: &str, replacement: &str) -> String {
        assert!(
            !needle.is_empty() && self.contains(needle),
            "fixture edit not found: {needle:?}"
        );
        self.replace(needle, replacement)
    }
}
