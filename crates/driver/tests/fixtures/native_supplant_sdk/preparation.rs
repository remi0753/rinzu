pub fn instrument(source: String, budget: Option<usize>) -> String {
    let arm = "int64_t rinzu_baseline_supplant_reserve(void *raw, int64_t plan_index, const uint64_t *arguments) {";
    let end = "void rinzu_baseline_supplant_start(void *raw, int64_t handle) {";
    assert_eq!(source.matches(arm).count(), 1);
    assert_eq!(source.matches(end).count(), 1);
    let budget=budget.map_or_else(||"const char *probe=getenv(\"RINZU_PREPARE_BUDGET\");rinzu_probe_budget=probe?strtol(probe,NULL,10):-1;".to_owned(),|budget|format!("rinzu_probe_budget={budget};"));
    let source=source.replace(arm,&format!("{arm}\n(void)rinzu_probe_realloc;if(!rinzu_probe_armed){{rinzu_probe_armed=1;{budget}rinzu_probe_initial=rinzu_probe_budget;}}"));
    let source=source.replace(end,&format!("{end}\nif(rinzu_probe_initial>=0){{printf(\"SUPPLANT_PREPARE_ALLOCATIONS=%ld\\n\",rinzu_probe_initial-rinzu_probe_budget);rinzu_probe_initial=-1;rinzu_probe_budget=-1;}}"));
    let prelude = r#"
#include <stdlib.h>
#include <stdio.h>
#include <unistd.h>
static long rinzu_probe_budget=-1,rinzu_probe_initial=-1;
static int rinzu_probe_armed;
static int rinzu_probe_allowed(void){if(!rinzu_probe_budget)return 0;if(rinzu_probe_budget>0)--rinzu_probe_budget;return 1;}
static void *rinzu_probe_malloc(size_t size){return rinzu_probe_allowed()?malloc(size):NULL;}
static void *rinzu_probe_calloc(size_t count,size_t size){return rinzu_probe_allowed()?calloc(count,size):NULL;}
static void *rinzu_probe_realloc(void *pointer,size_t size){return rinzu_probe_allowed()?realloc(pointer,size):NULL;}
#define malloc rinzu_probe_malloc
#define calloc rinzu_probe_calloc
#define realloc rinzu_probe_realloc
"#;
    format!("{prelude}\n{source}")
}
