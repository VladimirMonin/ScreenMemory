//! Private image worker supervisor; no library publication or database authority.
#[cfg(windows)]
mod windows;
mod wire;
#[cfg(windows)]
pub use windows::{Cancellation, TrustedWorker, WorkerError, WorkerResult};
pub use wire::{serve, Request, Response};
