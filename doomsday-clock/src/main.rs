#![feature(cell_update)]

assert_cfg!(all(
  not(all(
    feature = "tokio-runtime",
    feature = "async-std-runtime"
  )),
  not(all(feature = "tokio-runtime", feature = "smol-runtime")),
  not(all(feature = "async-std-runtime", feature = "smol-runtime")),
  any(
    feature = "tokio-runtime",
    feature = "async-std-runtime",
    feature = "smol-runtime"
  )
));

use std::{
  cell::Cell,
  future::Future,
  pin::Pin,
  sync::atomic::{AtomicBool, AtomicUsize, Ordering},
  task,
};

use async_local::{AsContext, AsyncLocal, Context, LocalRef};
use pin_project::{pin_project, pinned_drop};
use static_assertions::assert_cfg;
use tokio::sync::Notify;

const SECONDS_TO_MIDNIGHT: usize = 100;
const ARSENAL_SIZE: usize = 1600;

static CORE_ID: AtomicUsize = AtomicUsize::new(0);
static ARSENAL_ARMED: Notify = Notify::const_new();
static ARMAMENTS: AtomicUsize = AtomicUsize::new(0);
static CLOCK_DROPPED: AtomicBool = AtomicBool::new(false);

#[derive(AsContext)]
pub struct DoomsdayClock {
  core_id: usize,
  armed: Cell<usize>,
  disarmed: Context<AtomicUsize>,
}

impl DoomsdayClock {
  unsafe fn new() -> Self {
    let core_id = CORE_ID.fetch_add(1, Ordering::Release);

    DoomsdayClock {
      core_id,
      armed: Cell::new(0),
      disarmed: Context::new(AtomicUsize::new(0)),
    }
  }
}

impl Drop for DoomsdayClock {
  fn drop(&mut self) {
    CLOCK_DROPPED.fetch_or(true, Ordering::Release);

    let armed = self.armed.get();
    let disarmed = self.disarmed.load(Ordering::Acquire);

    match (disarmed, armed - disarmed) {
      (0, _) | (_, 0) => {
        if CORE_ID.fetch_sub(1, Ordering::AcqRel).eq(&1) {
          println!("It is {} seconds to midnight", SECONDS_TO_MIDNIGHT);
        }
      }
      (_, 1) => {
        println!("There is one warhead at doom's doorstep [{}]", self.core_id);
        panic!("The end is nigh");
      }
      (_, warheads) => {
        println!(
          "There are {} warheads at doom's doorstep [{}]",
          warheads, self.core_id
        );
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
  DoomsdayClock {
    clock: LocalRef<AtomicUsize>,
    core_id: usize,
  },
}

#[pin_project(PinnedDrop)]
pub struct NuclearWarhead {
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
    if CLOCK_DROPPED.load(Ordering::Acquire) {
      panic!("NulcearWarhead dropped after DoomsdayClock: dangling references will occur");
    }

    if let State::DoomsdayClock { clock, core_id: _ } = &self.state {
      clock.fetch_add(1, Ordering::Release);
    }
  }
}

impl Future for NuclearWarhead {
  type Output = ();
  fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
    let this = self.as_mut().project();

    match this.state {
      State::Proliferating => {
        let clock = unsafe { DOOMSDAY_CLOCK.local_ref() };

        let core_id = DOOMSDAY_CLOCK.with(|clock| {
          clock.armed.update(|n| n + 1);
          clock.core_id
        });

        let _ = std::mem::replace(this.state, State::DoomsdayClock { clock, core_id });

        cx.waker().wake_by_ref();

        task::Poll::Pending
      }
      State::DoomsdayClock { clock: _, core_id } => {
        if DOOMSDAY_CLOCK.with(|clock| clock.core_id).eq(core_id) {
          cx.waker().wake_by_ref();
        } else {
          let armed = ARMAMENTS.fetch_add(1, Ordering::Relaxed) + 1;

          if armed == ARSENAL_SIZE {
            ARSENAL_ARMED.notify_one();
          }
        }

        task::Poll::Pending
      }
    }
  }
}

#[cfg(feature = "tokio-runtime")]
#[tokio::main]
async fn main() {
  use tokio::task::yield_now;

  for _ in 0..ARSENAL_SIZE {
    tokio::task::spawn(async move {
      NuclearWarhead::proliferate().await;
    });
    yield_now().await;
  }

  ARSENAL_ARMED.notified().await;
  println!("At doom's doorstep:");
}

#[cfg(feature = "async-std-runtime")]
#[async_std::main]
async fn main() {
  for _ in 0..ARSENAL_SIZE {
    async_std::task::spawn(async move {
      NuclearWarhead::proliferate().await;
    });
    async_std::task::yield_now().await;
  }

  ARSENAL_ARMED.notified().await;
  println!("At doom's doorstep:");
}

#[cfg(feature = "smol-runtime")]
fn main() {
  use easy_parallel::Parallel;
  use smol::{channel::unbounded, future, Executor};

  let ex = Executor::new();
  let (signal, shutdown) = unbounded::<()>();

  Parallel::new()
    .each(0..num_cpus::get(), |_| {
      future::block_on(ex.run(shutdown.recv()))
    })
    .finish(|| {
      future::block_on(async {
        for _ in 0..ARSENAL_SIZE {
          ex.spawn(async move {
            NuclearWarhead::proliferate().await;
          })
          .detach();
          smol::future::yield_now().await;
        }

        ARSENAL_ARMED.notified().await;
        println!("At doom's doorstep:");
        drop(signal);
      })
    });
}
