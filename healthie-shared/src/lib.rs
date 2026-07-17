// Intentional conventions that conflict with clippy::pedantic:
#![allow(clippy::option_option, clippy::struct_field_names, clippy::wildcard_imports)]
// These pedantic lints target public-API surface. The domain library favours terse
// service signatures over per-fn doc/must_use annotations; allow them crate-wide to keep
// the `-D clippy::pedantic` gate green, matching the glovebox-shared reference crate.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::implicit_hasher
)]

pub mod clock;
pub mod entities;
pub mod error;
pub mod migration;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
