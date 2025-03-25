use std::{
  fmt::{self, Debug},
  io,
  sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
};

use linkme::distributed_slice;

use crate::{BarrierContext, CONTEXT};

#[derive(Default)]
struct ShutdownBarrier {
  guard_count: AtomicUsize,
  shutdown_finalized: Mutex<bool>,
  cvar: Condvar,
}

#[derive(PartialEq, Eq)]
pub(crate) enum Kind {
  CurrentThread,
  #[cfg(feature = "rt-multi-thread")]
  MultiThread,
}

#[doc(hidden)]
/// Builds Tokio runtime configured with a shutdown barrier
pub struct Builder {
  kind: Kind,
  worker_threads: usize,
  inner: tokio::runtime::Builder,
}

impl Builder {
  /// Returns a new builder with the current thread scheduler selected.
  pub fn new_current_thread() -> Builder {
    Builder {
      kind: Kind::CurrentThread,
      worker_threads: 1,
      inner: tokio::runtime::Builder::new_current_thread(),
    }
  }

  /// Returns a new builder with the multi thread scheduler selected.
  #[cfg(feature = "rt-multi-thread")]
  pub fn new_multi_thread() -> Builder {
    let worker_threads = std::env::var("TOKIO_WORKER_THEADS")
      .ok()
      .and_then(|worker_threads| worker_threads.parse().ok())
      .unwrap_or_else(num_cpus::get);

    Builder {
      kind: Kind::MultiThread,
      worker_threads,
      inner: tokio::runtime::Builder::new_multi_thread(),
    }
  }

  /// Enables both I/O and time drivers.
  pub fn enable_all(&mut self) -> &mut Self {
    self.inner.enable_all();
    self
  }

  /// Sets the number of worker threads the [`Runtime`] will use.
  ///
  /// This can be any number above 0 though it is advised to keep this value
  /// on the smaller side.
  ///
  /// This will override the value read from environment variable `TOKIO_WORKER_THREADS`.
  ///
  /// # Default
  ///
  /// The default value is the number of cores available to the system.
  ///
  /// When using the `current_thread` runtime this method has no effect.
  ///
  /// # Panics
  ///
  /// This will panic if `val` is not larger than `0`.
  #[track_caller]
  pub fn worker_threads(&mut self, val: usize) -> &mut Self {
    assert!(val > 0, "Worker threads cannot be set to 0");
    if self.kind.ne(&Kind::CurrentThread) {
      self.worker_threads = val;
      self.inner.worker_threads(val);
    }
    self
  }

  /// Creates a Tokio Runtime configured with a barrier that rendezvous worker threads during shutdown as to ensure tasks never outlive local data owned by worker threads
  pub fn build(&mut self) -> io::Result<Runtime> {
    let worker_threads = self.worker_threads;
    let barrier = Arc::new(ShutdownBarrier::default());

    let on_thread_start = {
      let barrier = barrier.clone();
      move || {
        let thread_count = barrier.guard_count.fetch_add(1, Ordering::Release);

        CONTEXT.with(|context| {
          if thread_count.ge(&worker_threads) {
            *context.borrow_mut() = Some(BarrierContext::PoolWorker)
          } else {
            *context.borrow_mut() = Some(BarrierContext::RuntimeWorker)
          }
        });
      }
    };

    let on_thread_stop = move || {
      let thread_count = barrier.guard_count.fetch_sub(1, Ordering::AcqRel);

      CONTEXT.with(|context| {
        if thread_count.eq(&1) {
          *barrier.shutdown_finalized.lock().unwrap() = true;
          barrier.cvar.notify_all();
        } else if context.borrow().eq(&Some(BarrierContext::RuntimeWorker)) {
          let mut shutdown_finalized = barrier.shutdown_finalized.lock().unwrap();
          while !*shutdown_finalized {
            shutdown_finalized = barrier.cvar.wait(shutdown_finalized).unwrap();
          }
        }
      });
    };

    self
      .inner
      .on_thread_start(on_thread_start)
      .on_thread_stop(on_thread_stop)
      .build()
      .map(Runtime::new)
  }
}

#[doc(hidden)]
pub struct Runtime(tokio::runtime::Runtime);

impl Debug for Runtime {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.0.fmt(f)
  }
}

impl Runtime {
  fn new(inner: tokio::runtime::Runtime) -> Self {
    Runtime(inner)
  }
  /// Runs a future to completion on the Tokio runtime. This is the
  /// runtime's entry point.
  ///
  /// This runs the given future on the current thread, blocking until it is
  /// complete, and yielding its resolved result. Any tasks or timers
  /// which the future spawns internally will be executed on the runtime.
  ///
  /// # Non-worker future
  ///
  /// Note that the future required by this function does not run as a
  /// worker. The expectation is that other tasks are spawned by the future here.
  /// Awaiting on other futures from the future provided here will not
  /// perform as fast as those spawned as workers.
  ///
  /// # Panics
  ///
  /// This function panics if the provided future panics, or if called within an
  /// asynchronous execution context.
  ///
  /// # Safety
  /// This is internal to async_local and is meant to be used exclusively with #[async_local::main] and #[async_local::test].
  #[track_caller]
  pub unsafe fn block_on<F: Future>(self, future: F) -> F::Output {
    unsafe { self.run(|handle| handle.block_on(future)) }
  }

  pub unsafe fn run<F, Output>(self, f: F) -> Output
  where
    F: for<'a> FnOnce(&'a tokio::runtime::Runtime) -> Output,
  {
    CONTEXT.with(|context| *context.borrow_mut() = Some(BarrierContext::Owner));

    let output = f(&self.0);

    drop(self);

    CONTEXT.with(|context| *context.borrow_mut() = None::<BarrierContext>);

    output
  }
}

#[doc(hidden)]
#[distributed_slice]
pub static RUNTIMES: [bool];

#[cfg(not(test))]
#[ctor::ctor]
fn assert_runtime_configured() {
  if RUNTIMES.ne(&[true]) {
    panic!(
      "The #[async_local::main] macro must be used to configure the Tokio runtime for use with the `async-local` crate. For compatibilty with other async runtime configurations, the `compat` feature can be used to disable the optimizations this crate provides"
    );
  }
}
