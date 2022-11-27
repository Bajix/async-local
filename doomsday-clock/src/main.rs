#![feature(cell_update)]

assert_cfg!(all(
  not(all(
    feature = "tokio-runtime",
    feature = "async-std-runtime"
  )),
  any(feature = "tokio-runtime", feature = "async-std-runtime")
));

use std::{
  cell::Cell,
  future::Future,
  pin::Pin,
  sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
  task,
};

use async_local::{AsyncLocal, Context, LocalRef};
use pin_project::{pin_project, pinned_drop};
use static_assertions::assert_cfg;
use tokio::sync::Notify;

const SECONDS_TO_MIDNIGHT: usize = 100;
static CORE_ID: AtomicUsize = AtomicUsize::new(0);
static ARSENAL_ARMED: Notify = Notify::const_new();
static ARMAMENTS: AtomicUsize = AtomicUsize::new(0);
pub struct DoomsdayClock {
  core_id: usize,
  seconds_to_midnight: Cell<usize>,
  warheads: Context<AtomicIsize>,
}

impl AsRef<Context<AtomicIsize>> for DoomsdayClock {
  fn as_ref(&self) -> &Context<AtomicIsize> {
    &self.warheads
  }
}

impl DoomsdayClock {
  unsafe fn new() -> Self {
    let core_id = CORE_ID.fetch_add(1, Ordering::Release);

    DoomsdayClock {
      core_id,
      seconds_to_midnight: Cell::new(SECONDS_TO_MIDNIGHT),
      warheads: unsafe { Context::new(AtomicIsize::new(SECONDS_TO_MIDNIGHT as isize)) },
    }
  }
}

impl Drop for DoomsdayClock {
  fn drop(&mut self) {
    let warheads = self.warheads.load(Ordering::Acquire);

    match warheads {
      0 => {
        if CORE_ID.fetch_sub(1, Ordering::AcqRel).eq(&1) {
          println!("It is {} seconds to midnight", SECONDS_TO_MIDNIGHT);
        }
      }
      1 => {
        println!("There is one warhead [{}]", self.core_id);
        panic!("The end is nigh");
      }
      _ => {
        println!("There are {} warheads at doom's doorstep", warheads,);
        panic!("The end is nigh");
      }
    }
  }
}

thread_local! {
  static DOOMSDAY_CLOCK: DoomsdayClock = unsafe { DoomsdayClock::new() };
}

#[pin_project]
enum State {
  Proliferating,
  DoomsdayClock(LocalRef<AtomicIsize>),
}

#[pin_project(PinnedDrop)]
struct NuclearWarhead {
  state: State,
}

impl NuclearWarhead {
  fn proliferate() -> Self {
    NuclearWarhead {
      state: State::Proliferating,
    }
  }
}

#[pinned_drop]
impl PinnedDrop for NuclearWarhead {
  fn drop(mut self: Pin<&mut Self>) {
    if let State::DoomsdayClock(clock) = &self.state {
      let seconds_to_midnight = clock.fetch_sub(1, Ordering::Release);

      if seconds_to_midnight <= 0 {
        panic!("The end is nigh")
      }
    }
  }
}

impl Future for NuclearWarhead {
  type Output = ();
  fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
    let this = self.as_mut().project();

    match this.state {
      State::Proliferating => {
        let arsenal_acquired = DOOMSDAY_CLOCK.with(|clock| clock.seconds_to_midnight.get().eq(&0));

        if !arsenal_acquired {
          DOOMSDAY_CLOCK.with(|clock| clock.seconds_to_midnight.update(|n| n.saturating_sub(1)));
          let clock = unsafe { DOOMSDAY_CLOCK.local_ref() };
          let _ = std::mem::replace(this.state, State::DoomsdayClock(clock));
          let armed = ARMAMENTS.fetch_add(1, Ordering::Relaxed) + 1;
          if armed == (SECONDS_TO_MIDNIGHT * num_cpus::get()) {
            ARSENAL_ARMED.notify_one();
          }
        } else {
          cx.waker().wake_by_ref();
        }

        task::Poll::Pending
      }
      State::DoomsdayClock(_) => task::Poll::Pending,
    }
  }
}

#[cfg(feature = "tokio-runtime")]
#[tokio::main]
async fn main() {
  for _ in 0..SECONDS_TO_MIDNIGHT * num_cpus::get() * 2 {
    tokio::task::spawn(async move {
      NuclearWarhead::proliferate().await;
    });
  }

  ARSENAL_ARMED.notified().await;
  println!("At doom's doorstep:");
}

#[cfg(feature = "async-std-runtime")]
#[async_std::main]
async fn main() {
  for _ in 0..SECONDS_TO_MIDNIGHT * num_cpus::get() * 2 {
    async_std::task::spawn(async move {
      NuclearWarhead::proliferate().await;
    });
  }

  ARSENAL_ARMED.notified().await;
  println!("At doom's doorstep:");
}
