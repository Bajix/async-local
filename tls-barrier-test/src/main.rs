use std::{
  ops::Deref,
  ptr::addr_of,
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
  thread::spawn,
  time::{Duration, Instant},
};

struct DropBarrier {
  dropped_at: Mutex<Option<Instant>>,
  cvar: Condvar,
}

static BARRIER: DropBarrier = DropBarrier {
  dropped_at: Mutex::new(None),
  cvar: Condvar::new(),
};

struct DropGuard;

thread_local! {
  static DROP_GUARD: DropGuard = DropGuard;
}

/// Should state impl Drop it will be invalidated regardless of a barrier blocking by the dtor function that's internally created by the thread_local macro
impl Drop for DropGuard {
  fn drop(&mut self) {
    let mut guard = BARRIER.dropped_at.lock().unwrap();

    *guard = Some(Instant::now());

    let (guard, _) = BARRIER
      .cvar
      .wait_timeout_while(guard, Duration::from_millis(100), |dropped_at| {
        dropped_at.is_some()
      })
      .unwrap();

    if guard.is_some() {
      println!("TLS protected by barrier");
    } else {
      panic!("TLS invalidated while guarded by barrier");
    }
  }
}

#[derive(Default)]

struct State(AtomicUsize);

impl Deref for State {
  type Target = AtomicUsize;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

struct StateRef(*const State);

impl Deref for StateRef {
  type Target = <State as Deref>::Target;
  fn deref(&self) -> &Self::Target {
    unsafe { (*self.0).deref() }
  }
}

unsafe impl Send for StateRef {}
unsafe impl Sync for StateRef {}

thread_local! {
  static STATE: State = State::default();
}

fn main() {
  DROP_GUARD.with(|_| {});

  let state = STATE.with(|state| {
    let addr = addr_of!(*state);

    state.store(addr as usize, Ordering::Release);

    StateRef(addr)
  });

  spawn(move || {
    let mut invalidated_at = Instant::now();

    while state.load(Ordering::Acquire).eq(&(state.0 as usize)) {
      invalidated_at = Instant::now();
    }

    let mut guard = BARRIER.dropped_at.lock().unwrap();

    if let Some(dropped_at) = guard.take() {
      let elapsed = invalidated_at.duration_since(dropped_at);
      println!("TLS invalided after {}ns", elapsed.as_nanos());
      BARRIER.cvar.notify_one();
    }
  });
}
