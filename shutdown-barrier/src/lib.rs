use std::{
  cell::RefCell,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
};

struct ShutdownBarrier {
  guard_count: AtomicUsize,
  shutdown_finalized: Mutex<bool>,
  cvar: Condvar,
}

impl ShutdownBarrier {
  const fn new() -> Self {
    ShutdownBarrier {
      guard_count: AtomicUsize::new(0),
      shutdown_finalized: Mutex::new(false),
      cvar: Condvar::new(),
    }
  }
}

static BARRIER: ShutdownBarrier = ShutdownBarrier::new();

#[derive(Default)]
struct ShutdownGuard {
  enabled: RefCell<bool>,
}

impl ShutdownGuard {
  fn guard_thread_shutdown(&self) {
    if self.enabled.borrow().eq(&false) {
      *self.enabled.borrow_mut() = true;
      BARRIER.guard_count.fetch_add(1, Ordering::Release);
    }
  }

  fn rendezvous(&self) {
    if self.enabled.borrow().eq(&true) {
      *self.enabled.borrow_mut() = false;
      if BARRIER.guard_count.fetch_sub(1, Ordering::AcqRel).eq(&1) {
        *BARRIER.shutdown_finalized.lock().unwrap() = true;
        BARRIER.cvar.notify_all();
      } else {
        let mut shutdown_finalized = BARRIER.shutdown_finalized.lock().unwrap();
        while !*shutdown_finalized {
          shutdown_finalized = BARRIER.cvar.wait(shutdown_finalized).unwrap();
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

/// Suspend the current thread until all guarded threads rendezvous during shutdown. A thread will be suspended at most once this way and only if previously guarded.
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
