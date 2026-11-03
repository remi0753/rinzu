use rinzu_compiler::NativeCliInputs;
use std::path::PathBuf;

#[derive(Default)]
pub struct PackageArguments {
    pub path: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub app_config: Option<PathBuf>,
    pub trace: bool,
    pub diagnostic_report: Option<PathBuf>,
    pub resource_report: Option<rinzu_runtime::resource_report::Config>,
    pub native: NativeCliInputs,
    pub input: crate::input::Options,
}

pub fn parse(
    command: &str,
    mut arguments: impl Iterator<Item = String>,
) -> Result<PackageArguments, String> {
    let mut result = PackageArguments::default();
    let mut report_path = None;
    let mut interval = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--language" | "--target"
                if matches!(command, "check" | "run" | "build" | "verify-aot") =>
            {
                let slot = if argument == "--language" {
                    &mut result.input.language
                } else {
                    &mut result.input.target
                };
                if slot.is_some() {
                    return Err(format!("duplicate {argument}"));
                }
                *slot = Some(value(&mut arguments, &argument)?);
            }
            "--" if matches!(command, "run" | "verify-aot") => {
                if arguments.next().is_some() {
                    return Err(
                        "program arguments are not supported in this language revision".into(),
                    );
                }
                break;
            }
            "-L" => result
                .native
                .search_paths
                .push(PathBuf::from(value(&mut arguments, "-L")?)),
            "-l" => result.native.libraries.push(value(&mut arguments, "-l")?),
            "--native-object" => result
                .native
                .objects
                .push(PathBuf::from(value(&mut arguments, "--native-object")?)),
            "--native-header" => result
                .native
                .headers
                .push(PathBuf::from(value(&mut arguments, "--native-header")?)),
            "--trace" if command == "run" => result.trace = true,
            "--diagnostic-report" if command == "run" => {
                if result.diagnostic_report.is_some() {
                    return Err("duplicate --diagnostic-report".into());
                }
                result.diagnostic_report =
                    Some(PathBuf::from(value(&mut arguments, "--diagnostic-report")?));
            }
            "--resource-report" if matches!(command, "run" | "test") => {
                report_path = Some(PathBuf::from(value(&mut arguments, "--resource-report")?));
            }
            "--resource-interval-ms" if matches!(command, "run" | "test") => {
                let text = value(&mut arguments, "--resource-interval-ms")?;
                if !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("--resource-interval-ms requires a non-negative integer".into());
                }
                interval = Some(
                    text.parse::<u64>()
                        .map_err(|error| format!("invalid resource interval: {error}"))?,
                );
            }
            "--output" if matches!(command, "build" | "bundle") => {
                if command == "bundle" && result.output.is_some() {
                    return Err("duplicate --output".into());
                }
                result.output = Some(PathBuf::from(value(&mut arguments, "--output")?))
            }
            "--config" if command == "bundle" => {
                if result.app_config.is_some() {
                    return Err("duplicate --config".into());
                }
                result.app_config = Some(PathBuf::from(value(&mut arguments, "--config")?));
            }
            _ if argument.starts_with("-L") && argument.len() > 2 => result
                .native
                .search_paths
                .push(PathBuf::from(&argument[2..])),
            _ if argument.starts_with("-l") && argument.len() > 2 => {
                result.native.libraries.push(argument[2..].to_owned())
            }
            _ if argument.starts_with('-') || argument.starts_with('@') => {
                return Err(format!("unsupported {command} flag `{argument}`"));
            }
            _ if result.path.is_none() => result.path = Some(PathBuf::from(argument)),
            _ => return Err(format!("unexpected argument `{argument}`")),
        }
    }
    if interval.is_some() && report_path.is_none() {
        return Err("--resource-interval-ms requires --resource-report".into());
    }
    result.resource_report = report_path.map(|path| rinzu_runtime::resource_report::Config {
        path,
        interval_ms: interval.unwrap_or(100),
    });
    Ok(result)
}

fn value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    arguments
        .next()
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_explicit_typed_inputs() {
        let result = parse(
            "check",
            [
                "-L",
                "native/lib",
                "pkg",
                "-lobjc",
                "--native-object",
                "native/shim.o",
                "--native-header",
                "native/api.h",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(result.path, Some(PathBuf::from("pkg")));
        assert_eq!(result.native.libraries, ["objc"]);
        assert_eq!(result.native.objects, [PathBuf::from("native/shim.o")]);
        assert_eq!(result.native.headers, [PathBuf::from("native/api.h")]);
        for args in [
            vec!["-Wl,-rpath,x"],
            vec!["@response", "other"],
            vec!["-L"],
            vec!["-l", "-evil"],
            vec!["--native-object", "--trace"],
            vec!["--native-header"],
        ] {
            assert!(parse("check", args.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn bundle_options_are_explicit_and_do_not_change_other_commands() {
        let parsed = parse(
            "bundle",
            [
                "app",
                "--config",
                "release.json",
                "--output",
                "Example.app",
                "-lSystem",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(parsed.app_config, Some(PathBuf::from("release.json")));
        assert_eq!(parsed.output, Some(PathBuf::from("Example.app")));
        assert_eq!(parsed.native.libraries, ["System"]);
        for command in ["check", "run", "build"] {
            assert!(
                parse(
                    command,
                    ["--config", "release.json"].into_iter().map(str::to_owned)
                )
                .is_err()
            );
        }
        for args in [
            vec!["--config"],
            vec!["--config", "one", "--config", "two"],
            vec!["--output", "one.app", "--output", "two.app"],
        ] {
            assert!(parse("bundle", args.into_iter().map(str::to_owned)).is_err());
        }
    }
}
