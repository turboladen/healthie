// Intentional conventions that conflict with clippy::pedantic:
#![allow(
    clippy::option_option,
    clippy::struct_field_names,
    clippy::wildcard_imports
)]
// Public service fns document their `# Errors`/`# Panics` explicitly (these
// become M1b tool-description source material), so those doc lints stay on.
// `implicit_hasher` targets generic HashMap params we don't expose; keep it
// allowed crate-wide.
#![allow(clippy::implicit_hasher)]

pub mod clock;
pub mod entities;
pub mod error;
pub mod inputs;
pub mod migration;
pub mod services;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
