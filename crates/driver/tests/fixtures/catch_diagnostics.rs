pub const SAVE: &str = r#"error WriteFailed {}
error CloseFailed {}

fn persist() {
    try {
        throw WriteFailed(message: "disk full");
    }
    finally {
        throw CloseFailed(message: "close failed");
    }
}

fn main() {
    try {
        persist();
    }
    catch [first: WriteFailed] {}
}
"#;
