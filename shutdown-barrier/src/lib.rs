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

static BARRIER: ShutdownBarrier = ShutdownBarrier {
  guard_count: AtomicUsize::new(0),
  shutdown_finalized: Mutex::new(false),
  cvar: Condvar::new(),
};

#[derive(PartialEq, Eq)]
enum Guard {
  Thread,
  Runtime,
}
#[derive(Default)]
struct ShutdownGuard(RefCell<Option<Guard>>);

impl ShutdownGuard {
  fn guard_thread_shutdown(&self) {
    if self.0.borrow().eq(&None) {
      BARRIER.guard_count.fetch_add(1, Ordering::Release);
      *self.0.borrow_mut() = Some(Guard::Thread);
    }
  }

  fn defer_shutdown(&self) {
    if self.0.borrow().eq(&None) {
      BARRIER.guard_count.fetch_add(1, Ordering::Release);
      *self.0.borrow_mut() = Some(Guard::Runtime);
    }
  }

  fn rendezvous(&self) {
    if self.0.borrow().ne(&None) {
      let guard_type = self.0.replace(None).unwrap();

      if BARRIER.guard_count.fetch_sub(1, Ordering::AcqRel).eq(&1) {
        *BARRIER.shutdown_finalized.lock().unwrap() = true;
        BARRIER.cvar.notify_all();
      } else if guard_type.eq(&Guard::Thread) {
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

/// Defer shutdown for the lifetime of the current thread
pub fn defer_shutdown() {
  SHUTDOWN_GUARD.try_with(ShutdownGuard::defer_shutdown).ok();
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
