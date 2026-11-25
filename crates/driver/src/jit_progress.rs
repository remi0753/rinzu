//! Human-readable progress for an explicitly scoped `run` command.
use std::borrow::Cow;
use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Auto,
    Always,
    Never,
}

impl Mode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err("--progress requires auto, always, or never".into()),
        }
    }

    fn enabled(self, stdout_terminal: bool, stderr_terminal: bool, ci: bool) -> bool {
        match self {
            Self::Auto => stdout_terminal && stderr_terminal && !ci,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

fn ci_environment() -> bool {
    [
        "CI",
        "CONTINUOUS_INTEGRATION",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "TF_BUILD",
        "JENKINS_URL",
        "TEAMCITY_VERSION",
        "BUILDKITE",
        "CIRCLECI",
        "TRAVIS",
    ]
    .iter()
    .any(|name| {
        std::env::var_os(name).is_some_and(|value| {
            !value.is_empty()
                && value != "0"
                && !value.to_string_lossy().eq_ignore_ascii_case("false")
        })
    })
}

struct Progress {
    started: Instant,
    main_started: Option<Instant>,
    phase: Cow<'static, str>,
}

impl Progress {
    fn write(&self, message: std::fmt::Arguments<'_>) {
        // A closed progress sink must not abort the application or change its
        // exit status. Each complete row is written under one stderr lock.
        let _ = writeln!(
            std::io::stderr().lock(),
            "[rinzu:jit] +{:.3}s {message}",
            self.started.elapsed().as_secs_f64()
        );
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Progress>> = const { RefCell::new(None) };
}

pub(crate) struct Session {
    previous: Option<Progress>,
}

impl Session {
    pub(crate) fn start(mode: Mode, path: &Path) -> Self {
        let enabled = mode.enabled(
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
            ci_environment(),
        );
        let progress = enabled.then(|| Progress {
            started: Instant::now(),
            main_started: None,
            phase: Cow::Borrowed("Preparing"),
        });
        if let Some(progress) = &progress {
            progress.write(format_args!("Preparing: {path:?}"));
        }
        Self {
            previous: CURRENT.replace(progress),
        }
    }

    pub(crate) fn finish(self, failed: bool, exit_code: u8) {
        CURRENT.with_borrow(|current| {
            if let Some(progress) = current {
                let execution = progress
                    .main_started
                    .map(|started| format!(", execution={:.3}s", started.elapsed().as_secs_f64()))
                    .unwrap_or_default();
                if failed {
                    progress.write(format_args!(
                        "Failed during {}: exit={exit_code}{execution}",
                        progress.phase
                    ));
                } else {
                    progress.write(format_args!("Finished: exit={exit_code}{execution}"));
                }
            }
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        CURRENT.set(self.previous.take());
    }
}

pub(crate) fn phase(name: &'static str) {
    CURRENT.with_borrow_mut(|current| {
        if let Some(progress) = current {
            progress.phase = Cow::Borrowed(name);
            progress.write(format_args!("{name}"));
        }
    });
}

pub(crate) fn frontend_progress(event: rinzu_compiler::FrontendProgress<'_>) {
    use rinzu_compiler::FrontendProgress;
    match event {
        FrontendProgress::ResolvingDependencies => phase("Resolving dependencies"),
        FrontendProgress::ParsingApplicationSources => phase("Parsing application sources"),
        FrontendProgress::CheckingApplicationTypes => phase("Checking application types"),
        FrontendProgress::PreparingAndVerifyingIr => phase("Preparing and verifying IR"),
        FrontendProgress::CheckingDependency(name) => {
            CURRENT.with_borrow_mut(|current| {
                if let Some(progress) = current {
                    let label = format!("Checking dependency: {}", name.escape_debug());
                    // Imports can re-enter a dependency while checking its own
                    // imports. Report resumed work, without duplicate rows.
                    if progress.phase != label {
                        progress.phase = Cow::Owned(label);
                        progress.write(format_args!("{}", progress.phase));
                    }
                }
            });
        }
    }
}

pub(crate) fn main_started() {
    CURRENT.with_borrow_mut(|current| {
        if let Some(progress) = current
            && progress.main_started.is_none()
        {
            let now = Instant::now();
            progress.main_started = Some(now);
            progress.phase = Cow::Borrowed("Running Main");
            progress.write(format_args!(
                "Starting Main (startup={:.3}s)",
                now.duration_since(progress.started).as_secs_f64()
            ));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::Mode;

    #[test]
    fn auto_requires_both_terminals_and_no_ci_while_explicit_modes_override() {
        for stdout in [false, true] {
            for stderr in [false, true] {
                for ci in [false, true] {
                    assert_eq!(
                        Mode::Auto.enabled(stdout, stderr, ci),
                        stdout && stderr && !ci
                    );
                    assert!(Mode::Always.enabled(stdout, stderr, ci));
                    assert!(!Mode::Never.enabled(stdout, stderr, ci));
                }
            }
        }
    }
}
