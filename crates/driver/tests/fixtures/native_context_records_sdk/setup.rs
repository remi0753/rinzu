pub fn create(root: &std::path::Path) {
    for name in ["app", "contexts"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("contexts/rinzu.toml"), "[package]\nname=\"contexts\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"context_app\"\nlanguage=\"0.2\"\n[dependencies]\ncontexts={path=\"../contexts\"}\n").unwrap();
    std::fs::write(
        root.join("contexts/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("contexts/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("contexts/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import contexts;import rinzu.stdio;fn execute()->Int{return contexts.exercise();}async fn main(){try{let value=execute();rinzu.stdio.writeLine(value);}catch errors{panic(\"unexpected context Error\");}}").unwrap();
}

pub fn create_results(root: &std::path::Path) {
    create(root);
    std::fs::write(
        root.join("contexts/src/api.rinzu"),
        format!(
            "{}\n{}",
            include_str!("api.rinzu"),
            include_str!("results.rinzu")
        ),
    )
    .unwrap();
    let header=include_str!("api.h").replace("#endif", "void *context_choose(void *,void *,int64_t (*)(void),int64_t);\nvoid *context_missing(void);\n#endif");
    std::fs::write(root.join("contexts/api.h"), header).unwrap();
    std::fs::write(root.join("contexts/api.c"),format!("{}\nvoid *context_choose(void *first,void *second,int64_t (*handler)(void),int64_t mode){{(void)handler();return mode==2?0:(mode==0?first:second);}}\nvoid *context_missing(void){{return 0;}}\n",include_str!("api.c"))).unwrap();
    let main = root.join("app/src/main.rinzu");
    let source = std::fs::read_to_string(&main)
        .unwrap()
        .replace("contexts.exercise()", "contexts.resultExercise()");
    std::fs::write(main, source).unwrap();
}

pub fn create_supplant(root: &std::path::Path, early: bool) {
    create(root);
    std::fs::write(root.join("contexts/src/api.rinzu"), format!(r#"{}
native {{
    supplant fn select(first:Context,second:Context,point:Packet,handler:fn(Context)->Int)->Context symbol "context_select";
    fn selectEntries()->Int symbol "context_select_entries";
    fn resetSelectEntries() symbol "context_select_reset";
}}
export Context;export Editor;export Packet;export Handle;
export make;export check;export change;export select;export selectEntries;export resetSelectEntries;
"#, include_str!("api.rinzu"))).unwrap();
    let header = include_str!("api.h").replace("#endif", "void *context_select(void *,void *,const struct Packet *,int64_t (*)(void *));\nint64_t context_select_entries(void);\nvoid context_select_reset(void);\n#endif");
    std::fs::write(root.join("contexts/api.h"), header).unwrap();
    std::fs::write(root.join("contexts/api.c"), format!(r#"{}
#include <stdlib.h>
static int64_t select_entries;
void *context_select(void *first,void *second,const struct Packet *point,int64_t (*handler)(void *)) {{
    (void)first;
    select_entries++;
    ((struct Packet *)point)->x+=7;
    if(handler(second)!=-11) abort();
    ((struct Packet *)point)->x+=5;
    return second;
}}
int64_t context_select_entries(void) {{return select_entries;}}
void context_select_reset(void) {{select_entries=0;}}
"#, include_str!("api.c"))).unwrap();
    std::fs::write(
        root.join("app/src/main.rinzu"),
        include_str!("supplant.rinzu").replace("EARLY", if early { "true" } else { "false" }),
    )
    .unwrap();
}
