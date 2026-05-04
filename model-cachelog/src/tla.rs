//! TLA+ model integration via tla-connect.
//!
//! Modules:
//! - [`value`] — parsing ITF values into model types
//! - [`trace`] — trace state types and `State` impl
//! - [`driver`] — `Driver` impl and state emission

pub mod driver;
pub mod trace;
pub mod value;
