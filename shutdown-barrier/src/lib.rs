#[cfg(loom)]
use std::ops::Deref;
#[cfg(not(loom))]
use std::{
  cell::UnsafeCell,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
};

#[cfg(loom)]
use loom::{
  cell::UnsafeCell,
  lazy_static,
  sync::Arc,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
  thread_local,
};

#[derive(Default)]
struct ShutdownBarrier {
  guard_count: AtomicUsize,
  shutdown_finalized: Mutex<bool>,
  cvar: Condvar,
}

impl ShutdownBarrier {
  #[cfg(not(loom))]
  const fn new() -> Self {
    ShutdownBarrier {
      guard_count: AtomicUsize::new(0),
      shutdown_finalized: Mutex::new(false),
      cvar: Condvar::new(),
    }
  }
}

struct ShutdownGuard {
  enabled: UnsafeCell<bool>,
  #[cfg(loom)]
  barrier: Arc<ShutdownBarrier>,
}

#[cfg(not(loom))]
impl Default for ShutdownGuard {
  fn default() -> Self {
    ShutdownGuard {
      enabled: UnsafeCell::new(false),
    }
  }
}

#[cfg(loom)]
impl Default for ShutdownGuard {
  fn default() -> Self {
    lazy_static! {
      static ref BARRIER: Arc<ShutdownBarrier> = Arc::new(ShutdownBarrier::default());
    }

    ShutdownGuard {
      enabled: UnsafeCell::new(false),
      barrier: BARRIER.clone(),
    }
  }
}

impl ShutdownGuard {
  #[cfg(not(loom))]
  fn barrier(&self) -> &ShutdownBarrier {
    static BARRIER: ShutdownBarrier = ShutdownBarrier::new();
    &BARRIER
  }

  #[cfg(loom)]
  fn barrier(&self) -> &ShutdownBarrier {
    self.barrier.deref()
  }

  #[cfg(not(loom))]
  unsafe fn with_enabled<F, R>(&self, f: F) -> R
  where
    F: FnOnce(*const bool) -> R,
  {
    f(self.enabled.get())
  }

  #[cfg(loom)]
  unsafe fn with_enabled<F, R>(&self, f: F) -> R
  where
    F: FnOnce(*const bool) -> R,
  {
    self.enabled.get().with(f)
  }

  #[cfg(not(loom))]
  unsafe fn with_enabled_mut<F, R>(&self, f: F) -> R
  where
    F: FnOnce(*mut bool) -> R,
  {
    f(self.enabled.get())
  }

  #[cfg(loom)]
  unsafe fn with_enabled_mut<F, R>(&self, f: F) -> R
  where
    F: FnOnce(*mut bool) -> R,
  {
    self.enabled.get_mut().with(f)
  }

  unsafe fn guard_thread_shutdown(&self) {
    if self.with_enabled(|enabled| (*enabled).eq(&false)) {
      self.with_enabled_mut(|enabled| *enabled = true);
      self.barrier().guard_count.fetch_add(1, Ordering::Release);
    }
  }

  unsafe fn rendezvous(&self) {
    if self.with_enabled(|enabled| (*enabled).eq(&true)) {
      self.with_enabled_mut(|enabled| *enabled = true);

      if self
        .barrier()
        .guard_count
        .fetch_sub(1, Ordering::AcqRel)
        .eq(&1)
      {
        *self.barrier().shutdown_finalized.lock().unwrap() = true;
        self.barrier().cvar.notify_all();
      } else {
        let mut shutdown_finalized = self.barrier().shutdown_finalized.lock().unwrap();
        while !*shutdown_finalized {
          shutdown_finalized = self.barrier().cvar.wait(shutdown_finalized).unwrap();
        }
      }
    }
  }
}

impl Drop for ShutdownGuard {
  fn drop(&mut self) {
    unsafe { self.rendezvous() };
  }
}

thread_local! {
  static SHUTDOWN_GUARD: ShutdownGuard = ShutdownGuard::default();
}

/// Guard current thread's local data destruction until guarded threads rendezvous during shutdown.
pub fn guard_thread_shutdown() {
  SHUTDOWN_GUARD.with(|guard| unsafe {
    guard.guard_thread_shutdown();
  });
}

#[cfg(all(test, loom))]
mod tests {
  use loom::thread;

  use super::*;

  fn reset_barrier() {
    SHUTDOWN_GUARD.with(|guard| {
      *guard.barrier().shutdown_finalized.lock().unwrap() = false;
    });
  }

  #[test]
  fn it_synchronizes_shutdown() {
    loom::model(|| {
      reset_barrier();

      let handles: Vec<_> = (0..2)
        .map(|_| {
          thread::spawn(|| {
            guard_thread_shutdown();
          })
        })
        .collect();

      for handle in handles {
        handle.join().unwrap();
      }
    });
  }
}
