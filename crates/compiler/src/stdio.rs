pub(crate) const FUNCTIONS: [&str; 2] = ["write", "writeLine"];

pub(crate) fn call(path: &[String]) -> Option<bool> {
    let name = if path.len() == 1 {
        path[0].strip_prefix("rinzu.stdio::")?
    } else if path.len() == 3 && path[0] == "rinzu" && path[1] == "stdio" {
        &path[2]
    } else {
        return None;
    };
    match name {
        "write" => Some(false),
        "writeLine" => Some(true),
        _ => None,
    }
}
