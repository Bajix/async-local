use std::{
  cell::RefCell,
  io,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
  },
};

use tokio::runtime::Runtime;

#[derive(Default)]
struct ShutdownBarrier {
  guard_count: AtomicUsize,
  shutdown_finalized: Mutex<bool>,
  cvar: Condvar,
}

#[derive(PartialEq, Eq, Debug)]
enum BarrierContext {
  /// Tokio Runtime Worker
  RuntimeWorker,
  /// Tokio Pool Worker
  PoolWorker,
}

thread_local! {
  static CONTEXT: RefCell<Option<BarrierContext>> = RefCell::new(None);
}

#[derive(PartialEq, Eq)]
pub(crate) enum Kind {
  CurrentThread,
  #[cfg(feature = "rt-multi-thread")]
  MultiThread,
}

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

  #[cfg(feature = "rt-multi-thread")]
  #[cfg_attr(docsrs, doc(cfg(feature = "rt-multi-thread")))]
  /// Returns a new builder with the multi thread scheduler selected.
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

  /// Sets the number of worker threads the `Runtime` will use.
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
  /// # Examples
  ///
  /// ## Multi threaded runtime with 4 threads
  ///
  /// ```
  /// use async_local::runtime;
  ///
  /// // This will spawn a work-stealing runtime with 4 worker threads.
  /// let rt = runtime::Builder::new_multi_thread()
  ///   .worker_threads(4)
  ///   .build()
  ///   .unwrap();
  ///
  /// rt.spawn(async move {});
  /// ```
  ///
  /// ## Current thread runtime (will only run on the current thread via `Runtime::block_on`)
  ///
  /// ```
  /// use async_local::runtime;
  ///
  /// // Create a runtime that _must_ be driven from a call
  /// // to `Runtime::block_on`.
  /// let rt = runtime::Builder::new_current_thread().build().unwrap();
  ///
  /// // This will run the runtime and future on the current thread
  /// rt.block_on(async move {});
  /// ```
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
  }
}
