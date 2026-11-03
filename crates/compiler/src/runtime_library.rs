//! Compiler-provided implementations of ordinary standard-library functions.
//!
//! These bindings are created after name and type checking. They have no source
//! declaration syntax; the reactor IDs belong to the private runtime interface.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFunction {
    ReadResourceSize,
    Sleep,
    NativeCompleted,
    NativeTransferred,
}

pub(crate) const TIMER_FUNCTIONS: [&str; 1] = ["sleep"];

impl RuntimeFunction {
    pub(crate) fn from_path(path: &[String]) -> Option<Self> {
        if matches!(path, [name] if name == "rinzu.timer::sleep")
            || matches!(path, [root, module, name] if root == "rinzu" && module == "timer" && name == "sleep")
        {
            return Some(Self::Sleep);
        }
        Some(match crate::core::function_name(path)? {
            "readResourceSize" => Self::ReadResourceSize,
            "nativeCompleted" => Self::NativeCompleted,
            "nativeTransferred" => Self::NativeTransferred,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ReadResourceSize => "rinzu.core::readResourceSize",
            Self::Sleep => "rinzu.timer::sleep",
            Self::NativeCompleted => "rinzu.core::nativeCompleted",
            Self::NativeTransferred => "rinzu.core::nativeTransferred",
        }
    }

    /// Versioned descriptor/bridge identity, never a source-level binding.
    pub fn service_id(self) -> &'static str {
        match self {
            Self::ReadResourceSize => "reactor.file-read-size",
            Self::Sleep => "reactor.timer",
            Self::NativeCompleted => "reactor.native-completion",
            Self::NativeTransferred => "reactor.native-transfer",
        }
    }
}
