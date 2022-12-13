#[cfg(not(loom))]
use std::{
  cell::Cell, sync::atomic::AtomicUsize, sync::atomic::Ordering, sync::Condvar, sync::Mutex,
};
#[cfg(loom)]
use std::{
  cell::Cell, sync::atomic::AtomicUsize, sync::atomic::Ordering, sync::Condvar, sync::Mutex,
};

struct ShutdownBarrier {
  runtime_worker_count: AtomicUsize,
  runtime_shutdown: Mutex<bool>,
  cvar: Condvar,
}

static BARRIER: ShutdownBarrier = ShutdownBarrier {
  runtime_worker_count: AtomicUsize::new(0),
  runtime_shutdown: Mutex::new(false),
  cvar: Condvar::new(),
};

#[derive(Default)]
struct ShutdownGuard {
  suspend_shutdown: Cell<bool>,
}

impl ShutdownGuard {
  fn guard_thread_shutdown(&self) {
    if self.suspend_shutdown.get().eq(&false) {
      BARRIER.runtime_worker_count.fetch_add(1, Ordering::Release);
      self.suspend_shutdown.set(true);
    }
  }

  fn rendezvous(&self) {
    if self.suspend_shutdown.get().eq(&true) {
      self.suspend_shutdown.set(false);
      if BARRIER
        .runtime_worker_count
        .fetch_sub(1, Ordering::AcqRel)
        .eq(&1)
      {
        *BARRIER.runtime_shutdown.lock().unwrap() = true;
        BARRIER.cvar.notify_all();
      } else {
        let mut runtime_shutdown = BARRIER.runtime_shutdown.lock().unwrap();
        while !*runtime_shutdown {
          runtime_shutdown = BARRIER.cvar.wait(runtime_shutdown).unwrap();
        }
      }
    }
  }
}

impl Drop for ShutdownGuard {
  fn drop(&mut self) {
    self.rendezvous();
  }
}

thread_local! {
  static SHUTDOWN_GUARD: ShutdownGuard = Default::default();
}

/// Guard current thread's local data destruction until guarded threads rendezvous during shutdown.
pub fn guard_thread_shutdown() {
  SHUTDOWN_GUARD
    .try_with(ShutdownGuard::guard_thread_shutdown)
    .ok();
}

/// Suspend the current thread until all guarded threads rendezvous during shutdown. A thread will be suspended at most once this way this way and only if previously guarded.
///
/// # Example
///
/// ```rust
/// pub struct Context<T: Sync>(T);
///
/// impl<T> Drop for Context<T>
/// where
///   T: Sync,
/// {
///   fn drop(&mut self) {
///     // By blocking here we ensure T cannot be deallocated until all guarded threads rendezvous while destroying thread local data
///     suspend_until_shutdown();
///   }
/// }
///
/// thread_local! {
///   static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
/// }
/// ```
pub fn suspend_until_shutdown() {
  SHUTDOWN_GUARD.try_with(ShutdownGuard::rendezvous).ok();
}
