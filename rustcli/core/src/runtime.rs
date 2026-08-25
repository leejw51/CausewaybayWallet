//! One tokio runtime, shared, and the single place the wallet blocks on it.
//!
//! The chain layer is `async` because reading a balance and moving funds are
//! the only things a wallet genuinely waits on, and doing four of them at once
//! — as `balance --all` does — should take as long as the slowest, not as long
//! as the sum. Everything above it stays synchronous: [`App::run`] is a
//! function from a command to output, the CLI has no `#[tokio::main]`, and the
//! C ABI does not ask its host to own a reactor.
//!
//! [`block_on`] is the seam between the two, and it is deliberately the only
//! one. A second runtime built somewhere else would deadlock the first the
//! moment they nested.
//!
//! [`App::run`]: crate::app::App::run

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

use crate::error::{self, Result};

/// The process-wide runtime, built on first use.
///
/// Multi-threaded rather than current-thread: Midnight's proving is CPU-bound
/// work handed to [`tokio::task::spawn_blocking`], and a current-thread
/// runtime would have nowhere to put it.
fn runtime() -> Result<&'static Runtime> {
    static RUNTIME: OnceLock<std::result::Result<Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            Builder::new_multi_thread()
                .enable_all()
                .thread_name("causewaybay-chain")
                // A wallet issues a handful of concurrent requests, not
                // thousands. Four keeps the footprint small and is still more
                // than the number of chains.
                .worker_threads(4)
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| error::internal(format!("cannot start the async runtime: {e}")))
}

/// Run a future to completion on the shared runtime.
///
/// # Panics
///
/// Never, but it will return an error rather than deadlock if called from
/// inside the runtime it would block on. That only happens if a chain
/// implementation calls back into a synchronous command, which is a bug in
/// that chain rather than something a caller can provoke.
pub fn block_on<F: Future>(future: F) -> Result<F::Output> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(error::internal(
            "block_on was called from inside the async runtime; a chain must \
             await its work rather than re-entering a synchronous command",
        ));
    }
    Ok(runtime()?.block_on(future))
}

/// Run a closure on the blocking pool, for work that would stall the reactor.
///
/// Midnight's ZK proving is seconds of CPU with no await points in it; leaving
/// it on a worker thread would stop every other chain's request for the
/// duration.
pub async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| error::internal(format!("a background task failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_runs_to_completion() {
        assert_eq!(block_on(async { 2 + 2 }).unwrap(), 4);
    }

    #[test]
    fn the_runtime_is_shared_across_calls() {
        // Two blocks in a row must not each try to build their own.
        assert_eq!(block_on(async { "first" }).unwrap(), "first");
        assert_eq!(block_on(async { "second" }).unwrap(), "second");
    }

    #[test]
    fn concurrent_work_overlaps_rather_than_queueing() {
        use std::time::{Duration, Instant};
        let started = Instant::now();
        block_on(async {
            let a = tokio::time::sleep(Duration::from_millis(120));
            let b = tokio::time::sleep(Duration::from_millis(120));
            let c = tokio::time::sleep(Duration::from_millis(120));
            tokio::join!(a, b, c);
        })
        .unwrap();
        // Three 120ms waits in parallel finish in well under their sum.
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "took {:?}; the three waits did not overlap",
            started.elapsed()
        );
    }

    #[test]
    fn blocking_work_runs_off_the_reactor() {
        let out = block_on(async { blocking(|| (1..=10u64).sum::<u64>()).await }).unwrap();
        assert_eq!(out.unwrap(), 55);
    }

    #[test]
    fn re_entering_the_runtime_is_an_error_rather_than_a_deadlock() {
        // The failure this guards is unrecoverable in production: a nested
        // block_on parks a worker thread forever and the command never returns.
        let nested = block_on(async { block_on(async { 1 }) }).unwrap();
        let err = nested.unwrap_err();
        assert_eq!(err.code, error::Code::Internal);
        assert!(err.message.contains("re-entering"), "{}", err.message);
    }
}
