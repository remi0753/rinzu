use super::*;
use crate::FileId;

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(17), "style.rinzu", text)
}

fn spellings(file: &SourceFile) -> Vec<String> {
    let bom = file.original_offset(0);
    lex(file)
        .unwrap()
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .map(|token| {
            file.text()[token.range.start_byte as usize - bom..token.range.end_byte as usize - bom]
                .to_owned()
        })
        .collect()
}

fn formatted(text: &str) -> String {
    let input = file(text);
    let once = format_source(&input).unwrap();
    let output = file(&once);
    assert_eq!(
        spellings(&input),
        spellings(&output),
        "token spellings changed"
    );
    assert_eq!(
        format_source(&output).unwrap(),
        once,
        "formatting is unstable"
    );
    once
}

#[test]
fn declarations_have_one_blank_line_while_imports_and_exports_stay_grouped() {
    let text = formatted(
        "import rinzu.stdio;\n\n\nimport rinzu.timer;fn first(){}\n\n\n\nasync fn second(){}export first;\n\nexport second;class Empty{}",
    );
    assert_eq!(
        text,
        "import rinzu.stdio;\nimport rinzu.timer;\n\nfn first() {}\n\nasync fn second() {}\n\nexport first;\nexport second;\n\nclass Empty {}\n"
    );
}

#[test]
fn interface_bounds_self_and_error_clauses_keep_generic_delimiters_and_comments() {
    let text = formatted(
        "// requirements\ninterface Reader<T interface Codec<Map<String,Int>>>{// instance\nfn read(self:Self,values:List<Self>)->Map<String,List<T>> throws ParseFailed<String,Int>,ReadFailed;async fn close(self:Self)nothrow;}class Buffer<T:Transferable interface Codec<Int>+Named> interface Reader<T>+Named{const value:T fn value(self:Buffer<T>)->T nothrow{return self.value;}}fn main()nothrow{}",
    );
    within_width_or_preserved(&file(&text));
    assert!(text.contains("interface Reader<"), "{text}");
    assert!(
        text.contains("throws ParseFailed<String, Int>, ReadFailed;"),
        "{text}"
    );
    assert!(
        text.contains("async fn close(self: Self) nothrow;"),
        "{text}"
    );
    assert!(text.contains("// instance"));
}

#[test]
fn class_and_native_functions_are_separated_without_splitting_fields() {
    let text = formatted(
        "class Counter{const label:String,value:Int fn read(self:Counter)->Int{return self.value;}private async fn reset(self:Counter){self.value=0;}}native{opaque Handle;fn first(handle:Handle) symbol \"first\";fn second(handle:Handle) symbol \"second\";}",
    );
    assert!(
        text.contains("    const label: String,\n    value: Int\n\n    fn read("),
        "{text}"
    );
    assert!(
        text.contains("    }\n\n    private async fn reset("),
        "{text}"
    );
    assert!(text.contains("    opaque Handle;\n\n    fn first(handle: Handle) symbol \"first\";\n\n    fn second(handle: Handle) symbol \"second\";\n}"), "{text}");
    assert!(!text.contains("\n\n}"), "{text}");
}

#[test]
fn declaration_spacing_keeps_doc_comments_attached_and_closures_in_their_statements() {
    let text = formatted(
        "import rinzu.stdio;\n/// First function.\nfn first(){}\n/// Entry point.\nfn main(){let firstValue=fn(){};let secondValue=async fn(){};}",
    );
    assert!(
        text.contains("import rinzu.stdio;\n\n/// First function.\nfn first()"),
        "{text}"
    );
    assert!(
        text.contains("fn first() {}\n\n/// Entry point.\nfn main()"),
        "{text}"
    );
    assert!(
        text.contains("let firstValue = fn () {};\n    let secondValue = async fn () {};"),
        "{text}"
    );
}

fn within_width(text: &str) {
    for (index, line) in text.lines().enumerate() {
        assert!(
            line.chars().count() <= MAX_WIDTH,
            "line {} has {} characters: {line}",
            index + 1,
            line.chars().count()
        );
        assert_eq!(line, line.trim_end(), "formatter added trailing whitespace");
    }
}

fn within_width_or_preserved(file: &SourceFile) {
    let tokens = lex(file).unwrap();
    let mut start = 0;
    for (index, line) in file.text().split_inclusive('\n').enumerate() {
        let end = start + line.trim_end_matches(['\r', '\n']).len();
        let indent = line
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        if line.trim_end_matches(['\r', '\n']).chars().count() > MAX_WIDTH {
            let preserved_token = tokens.iter().any(|token| {
                if !matches!(
                    token.kind,
                    TokenKind::Identifier(_)
                        | TokenKind::String(_)
                        | TokenKind::FormattedString(_)
                        | TokenKind::Bytes(_)
                ) {
                    return false;
                }
                let left = start.max(token.range.start_byte as usize);
                let right = end.min(token.range.end_byte as usize);
                left < right && file.text()[left..right].chars().count() + indent + 2 > MAX_WIDTH
            });
            let mut previous = 0;
            let comment = tokens.iter().any(|token| {
                let left = start.max(previous);
                let right = end.min(token.range.start_byte as usize);
                previous = token.range.end_byte as usize;
                left < right && !file.text()[left..right].trim().is_empty()
            });
            assert!(
                preserved_token || comment,
                "unjustified overflow at line {}: {line}",
                index + 1
            );
        }
        start += line.len();
    }
}

#[test]
fn logical_negation_is_tight_but_not_equal_keeps_operator_spacing() {
    let text =
        formatted("fn main(){let a=! true;let b= ! (a&&false);let c= ! !a;if ! a && a!=b {}} ");
    assert_eq!(
        text,
        "fn main() {\n    let a = !true;\n    let b = !(a && false);\n    let c = !!a;\n    if !a && a != b {}\n}\n"
    );
    let comments = formatted("fn main(){let value=! /* why */ true;}");
    assert!(comments.contains("/* why */"), "{comments}");
}

#[test]
fn unary_minus_is_tight_but_subtraction_keeps_operator_spacing() {
    let text = formatted(
        "fn missing()->Int fallback - 17 {return -17;} fn main(){let a= - 17;let b= - (a+1);let c=a- -2;let d= - -a;}",
    );
    assert_eq!(
        text,
        "fn missing() -> Int fallback -17 {\n    return -17;\n}\n\nfn main() {\n    let a = -17;\n    let b = -(a + 1);\n    let c = a - -2;\n    let d = --a;\n}\n"
    );
    let comments = formatted("fn main(){let value=- /* why */ 17;}");
    assert!(comments.contains("/* why */"), "{comments}");
}

#[test]
fn empty_braces_stay_inline_in_declarations_and_control_flow() {
    let text = formatted(
        "error E {\n}\nclass Empty { }\nnative {\n}\nfn noop() {\n}\n\
         fn main(){if true { } else {\n}\nwhile false {}\n\
         try {} catch errors {} finally {}\n\
         switch value { case Option.None: {} case Option.Some(value): {} }}",
    );
    assert_eq!(
        text,
        "error E {}\n\nclass Empty {}\n\nnative {}\n\nfn noop() {}\n\n\
         fn main() {\n    if true {}\n    else {}\n    while false {}\n    try {}\n    catch errors {}\n    finally {}\n    switch value {\n        case Option.None: {}\n        case Option.Some(value): {}\n    }\n}\n"
    );
}

#[test]
fn braces_with_comments_keep_their_contents_on_separate_lines() {
    let text = formatted(
        "error E {/* explanation */}\nfn main(){if true {// keep\n} else {\n/* keep too */\n}}",
    );
    assert!(
        text.contains("error E {\n    /* explanation */\n}"),
        "{text}"
    );
    assert!(text.contains("if true {\n        // keep\n    }"), "{text}");
    assert!(
        text.contains("else {\n        /* keep too */\n    }"),
        "{text}"
    );
    assert!(!text.contains("{}"), "{text}");
}

#[test]
fn generics_are_tight_in_declarations_types_calls_and_nested_arguments() {
    let text = formatted(
        "class Box < T > {value:T} enum Choice<T>{Some(value:T),None}\n\
         type Items<T> = Map < String, List < Box < T > > >;\n\
         fn identity<T:Eq+Show,U>(value:T,unused:U)->T{return value;}\n\
         fn main(){let values:List<Map<String,Box<Int>>> = [];\n\
         let result=identity<List<Box<Int>>,Int>([],1);\n\
         let trailing=identity<Int,Int,>(1,2);\n\
         let choice=Choice<Box<Int>>.Some(value:Box<Int>(value:1));}",
    );
    for expected in [
        "class Box<T>",
        "enum Choice<T>",
        "type Items<T> = Map<String, List<Box<T>>>;",
        "identity<T: Eq + Show, U>(",
        "values: List<Map<String, Box<Int>>>",
        "identity<List<Box<Int>>, Int>([], 1)",
        "identity<Int, Int,>(1, 2)",
        "Choice<Box<Int>>.Some(value: Box<Int>(value: 1))",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    within_width(&text);
}

#[test]
fn comparisons_keep_operator_spacing_and_do_not_become_generics() {
    let text = formatted(
        "fn compare(a:Int,b:Int,c:Int){\n\
         if a<b && b>c || a<=c && b>=c {}\n\
         let apparent=a<b>c;let actual=identity<Int>(a);}",
    );
    assert!(
        text.contains("a < b && b > c || a <= c && b >= c"),
        "{text}"
    );
    assert!(text.contains("apparent = a < b > c;"), "{text}");
    assert!(text.contains("actual = identity<Int>(a);"), "{text}");
}

#[test]
fn class_fields_and_native_record_fields_are_vertical_before_methods() {
    let text = formatted(
        "class Store<T>{const table:Map<String,List<T>>,\n\
         callback:fn(Int,String)->Int,value:Int\n\
         fn read(self:Store<T>)->Int{return self.value;}}\n\
         native{record Pair{left:CInt,right:CInt}}",
    );
    assert!(
        text.contains("    const table: Map<String, List<T>>,\n"),
        "{text}"
    );
    assert!(
        text.contains("    callback: fn (Int, String) -> Int,\n"),
        "{text}"
    );
    assert!(text.contains("    value: Int\n\n    fn read("), "{text}");
    assert!(
        text.contains("        left: CInt,\n        right: CInt\n"),
        "{text}"
    );
    within_width(&text);
}

#[test]
fn native_roles_format_with_memberless_opaque_and_class_operations() {
    let text = formatted(
        "native{opaque Handle;record Packet{value:Int}}type Context={handle:Handle};class Owner{const handle:Handle,const packet:Packet fn read(self:Owner)->Int{return self.packet.value;}}",
    );
    assert!(text.contains("opaque Handle;"), "{text}");
    assert!(
        text.contains("record Packet {\n        value: Int\n"),
        "{text}"
    );
    assert!(text.contains("class Owner {"), "{text}");
    assert!(text.contains("fn read(self: Owner)"), "{text}");
    for old in ["native{type Handle;}", "native{userdata Context;}"] {
        assert!(
            crate::format_source(&crate::SourceFile::new(crate::FileId(1), "old.rinzu", old))
                .is_err()
        );
    }
}

#[test]
fn enum_variants_break_after_members_but_keep_short_payloads_inline() {
    let text =
        formatted("enum Outcome<T,U>{Ready(value:T,details:U),Failed(problem:String),Empty,}");
    assert_eq!(
        text,
        "enum Outcome<T, U> {\n    Ready(value: T, details: U),\n    Failed(problem: String),\n    Empty,\n}\n"
    );
}

#[test]
fn long_signatures_calls_lists_types_and_binary_expressions_wrap() {
    let text = formatted(
        "type Details = Map<FirstVeryLongButIndividualTypeName,Map<SecondVeryLongButIndividualTypeName,List<ThirdVeryLongButIndividualTypeName>>>;\n\
         fn combine(firstLongParameterName:Details,secondLongParameterName:Details,thirdLongParameterName:Details,fourthLongParameterName:Details)->Details{return firstLongParameterName;}\n\
         fn main(){let result=combine(firstLongArgumentExpressionName,secondLongArgumentExpressionName,thirdLongArgumentExpressionName);\n\
         let values=[firstLongArgumentExpressionName,secondLongArgumentExpressionName,thirdLongArgumentExpressionName,fourthLongArgumentExpressionName];\n\
         let sum=firstLongArgumentExpressionName+secondLongArgumentExpressionName+thirdLongArgumentExpressionName+fourthLongArgumentExpressionName;\n\
         let chain=firstLongArgumentExpressionName.secondLongArgumentExpressionName.thirdLongArgumentExpressionName.fourthLongArgumentExpressionName();}",
    );
    within_width(&text);
    for expected in [
        "fn combine(\n",
        "combine(\n",
        "[\n",
        "\n        + ",
        "\n        .",
    ] {
        assert!(
            text.contains(expected),
            "missing break {expected:?}:\n{text}"
        );
    }
}

#[test]
fn width_boundary_counts_unicode_code_points_and_includes_indentation() {
    let prefix = "    let value = \"";
    let suffix = "\";";
    for width in [119, 120, 121] {
        let value = "界".repeat(width - prefix.chars().count() - suffix.chars().count());
        let text = formatted(&format!("fn main(){{let value=\"{value}\";}}"));
        let line = format!("{prefix}{value}{suffix}");
        assert_eq!(
            text.lines().any(|actual| actual == line),
            width <= MAX_WIDTH,
            "{text}"
        );
        within_width(&text);
    }
}

#[test]
fn assignment_rhs_literals_fit_without_the_operator_on_their_continuation_line() {
    for nested in [false, true] {
        let indent = if nested { 3 * INDENT } else { 2 * INDENT };
        for width in MAX_WIDTH - indent - 3..=MAX_WIDTH - indent - 1 {
            let interpolation = "f\"{value}\"";
            for literal in [
                format!("\"{}\"", "界".repeat(width - 2)),
                format!("b\"{}\"", "x".repeat(width - 3)),
                format!(
                    "f\"{{value}}{}\"",
                    "x".repeat(width - interpolation.chars().count())
                ),
            ] {
                let statement = format!("let next={literal};value={literal};");
                let body = if nested {
                    format!("if true{{{statement}}}")
                } else {
                    statement
                };
                let text = formatted(&format!("fn render(value:String){{{body}}}"));
                within_width(&text);
                let expected = format!("{}{literal};", " ".repeat(indent));
                assert_eq!(
                    text.lines().filter(|line| *line == expected).count(),
                    2,
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn comments_and_indivisible_literals_are_preserved_even_beyond_the_width() {
    let long = "x".repeat(150);
    let comment = format!("// keep {long}");
    let raw = format!("r##\"{long}\r\n\t\\u{{0}}\0\"##");
    let interpolation = "f\"{identity<List<Int>>([])} {1<2}\"";
    let text = formatted(&format!(
        "{comment}\nfn main(){{let raw={raw};let bytes=b\"\\x00\\xff\";let text={interpolation};}}"
    ));
    for preserved in [&comment, &raw, interpolation] {
        assert!(
            text.contains(preserved),
            "missing literal/comment {preserved:?}"
        );
    }
    assert!(text.lines().any(|line| line.chars().count() > MAX_WIDTH));
}

#[test]
fn nested_blocks_and_comments_inside_delimiters_keep_tokens_and_stable_indentation() {
    let text = formatted(
        "fn main(){let result=apply(fn(value:Int)->Int{/* body */return value+1;},\n\
         /* argument */2);let item={first=1,second=2};\n\
         let list=[/* first */item,/* second */item];}\n\
         enum Value{Ready(value:{first:Int,second:Int}),Empty}",
    );
    within_width(&text);
    assert!(
        text.contains("},"),
        "block comma must stay attached: {text}"
    );
    assert!(
        text.contains("};"),
        "block semicolon must stay attached: {text}"
    );
}

#[test]
fn long_field_annotations_and_generic_constraints_wrap_at_their_boundaries() {
    let name = "field".repeat(18);
    let ty = "Type".repeat(22);
    let constraint = "Property".repeat(8);
    let text = formatted(&format!(
        "class Store<T:{constraint}+{constraint}>{{{name}:{ty}}}\n\
         fn accept({name}:{ty}){{let item=Store<Int>({name}:{name});}}"
    ));
    within_width(&text);
    assert!(text.contains(&format!("{name}:\n")), "{text}");
}

#[test]
fn conditions_keep_short_terms_intact_after_nested_calls_wrap() {
    let text = formatted(
        "fn main(){if packet.narrow!=CInt8(-128)||packet.wide!=wide||!packet.okay||packet.small==packet.small||packet.large!=large||small!=CFloat(-0.0)||large!=42.5||!okay{panic(\"bad\");}\n\
         let result=veryLongCallName(firstLongArgumentExpressionName,secondLongArgumentExpressionName,thirdLongArgumentExpressionName)+otherLongArgumentExpressionName;}",
    );
    within_width(&text);
    assert!(text.contains("packet.narrow != CInt8(-128)"), "{text}");
    assert!(text.contains("\n        || packet.wide != wide"), "{text}");
    assert!(text.contains("otherLongArgumentExpressionName"), "{text}");
}

#[test]
fn native_binding_can_wrap_even_when_the_signature_has_no_parameters() {
    let name = "Name".repeat(25);
    let symbol = "native_symbol_".repeat(4);
    let text = formatted(&format!("native{{fn {name}() symbol \"{symbol}\";}}"));
    within_width(&text);
    assert!(
        text.contains(&format!("{name}()\n        symbol \"{symbol}\";")),
        "{text}"
    );
}

#[test]
fn repository_sources_preserve_token_spellings_and_format_idempotently() {
    fn paths(root: &std::path::Path, output: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                if !name.starts_with('.')
                    && !matches!(
                        name.as_ref(),
                        "target" | "build" | "artifacts" | "node_modules"
                    )
                {
                    paths(&entry.path(), output);
                }
            } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "rinzu") {
                output.push(entry.path());
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut sources = Vec::new();
    paths(&root, &mut sources);
    sources.sort();
    let mut checked = 0;
    let mut rejected = 0;
    for path in sources {
        let input =
            SourceFile::from_utf8(FileId(17), &path, std::fs::read(&path).unwrap()).unwrap();
        if parse_source(&input).is_err() {
            // A syntax-error fixture is not writable by the formatter; CLI tests cover rejection.
            rejected += 1;
            continue;
        }
        let once = format_source(&input).unwrap();
        let output = file(&once);
        assert_eq!(
            spellings(&input),
            spellings(&output),
            "tokens: {}",
            path.display()
        );
        assert_eq!(
            format_source(&output).unwrap(),
            once,
            "idempotence: {}",
            path.display()
        );
        within_width_or_preserved(&output);
        checked += 1;
    }
    assert!(checked > 0);
    println!(
        "repository formatter audit: {checked} valid sources, {rejected} syntax-error fixtures"
    );
}
