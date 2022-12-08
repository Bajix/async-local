#![cfg_attr(not(feature = "boxed"), feature(type_alias_impl_trait))]
#![cfg_attr(test, feature(exit_status_error))]

extern crate self as async_local;

#[cfg(not(loom))]
use std::{
  cell::Cell, sync::atomic::AtomicUsize, sync::atomic::Ordering, sync::Condvar, sync::Mutex,
  thread::LocalKey,
};
#[cfg(loom)]
use std::{
  cell::Cell, sync::atomic::AtomicUsize, sync::atomic::Ordering, sync::Condvar, sync::Mutex,
  thread::LocalKey,
};
use std::{future::Future, marker::PhantomData, ops::Deref, ptr::addr_of};

pub use derive_async_local::AsContext;

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

struct ContextGuard {
  sync_on_drop: Cell<bool>,
}

thread_local! {
  static CONTEXT_GUARD: ContextGuard = ContextGuard { sync_on_drop: Cell::new(false) };
}

impl ContextGuard {
  fn enable_shutdown_synchronization() {
    CONTEXT_GUARD
      .try_with(|guard| {
        if guard.sync_on_drop.get().eq(&false) {
          BARRIER.runtime_worker_count.fetch_add(1, Ordering::Release);
          guard.sync_on_drop.set(true);
        }
      })
      .ok();
  }

  fn sync_shutdown(&self) {
    if self.sync_on_drop.get().eq(&true) {
      self.sync_on_drop.set(false);
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

impl Drop for ContextGuard {
  fn drop(&mut self) {
    self.sync_shutdown();
  }
}

/// A wrapper around a thread-safe inner type used for creating pointers to thread-locals that are valid within an async context
pub struct Context<T: Sync>(T);

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// # Usage
  ///
  /// Either wrap an inner type with Context and assign to a thread-local, or use as an unwrapped field in a struct that derives [AsContext]
  ///
  /// # Example
  ///
  /// ```rust
  /// thread_local! {
  ///   static COUNTER: Context<AtomicUsize> = Context::new(|| AtomicUsize::new(0));
  /// }
  /// ```
  pub fn new(inner: T) -> Context<T> {
    Context(inner)
  }
}

impl<T> AsRef<Context<T>> for Context<T>
where
  T: Sync,
{
  fn as_ref(&self) -> &Context<T> {
    self
  }
}

impl<T> Deref for Context<T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl<T> Drop for Context<T>
where
  T: Sync,
{
  fn drop(&mut self) {
    CONTEXT_GUARD.try_with(ContextGuard::sync_shutdown).ok();
  }
}

/// A marker trait promising [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is safely implemented
///
/// # Safety
///
/// When assigned to a thread local, the [pin drop guarantee](https://doc.rust-lang.org/std/pin/index.html#drop-guarantee) must be upheld for [`Context<T>`]: it cannot be wrapped in a pointer type nor cell type and it must not be invalidated nor repurposed until when [drop](https://doc.rust-lang.org/std/ops/trait.Drop.html#tymethod.drop) happens as a consequence of the thread dropping.
pub unsafe trait AsContext<T>: AsRef<Context<T>>
where
  T: Sync,
{
}

unsafe impl<T> AsContext<T> for Context<T> where T: Sync {}

/// A thread-safe pointer to a thread local [`Context`]
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalRef<T: Sync + 'static>(*const Context<T>);

impl<T> LocalRef<T>
where
  T: Sync + 'static,
{
  unsafe fn new(context: &Context<T>) -> Self {
    ContextGuard::enable_shutdown_synchronization();

    LocalRef(addr_of!(*context))
  }

  /// # Safety
  ///
  /// To ensure that it is not possible for [`RefGuard`] to be moved to a thread outside of the async runtime, this must be constrained to any non-`'static` lifetime such as `'async_trait`
  pub unsafe fn guarded_ref<'a>(&self) -> RefGuard<'a, T> {
    RefGuard {
      inner: self.0,
      _marker: PhantomData,
    }
  }
}

impl<T> Deref for LocalRef<T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    unsafe { (*self.0).deref() }
  }
}

impl<T> Clone for LocalRef<T>
where
  T: Sync + 'static,
{
  fn clone(&self) -> Self {
    LocalRef(self.0)
  }
}

impl<T> Copy for LocalRef<T> where T: Sync + 'static {}

unsafe impl<T> Send for LocalRef<T> where T: Sync {}
unsafe impl<T> Sync for LocalRef<T> where T: Sync {}

/// A thread-safe pointer to a thread-local [`Context`] constrained by a phantom lifetime
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RefGuard<'a, T: Sync + 'static> {
  inner: *const Context<T>,
  _marker: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<'a, T> RefGuard<'a, T>
where
  T: Sync + 'static,
{
  unsafe fn new(context: &Context<T>) -> Self {
    ContextGuard::enable_shutdown_synchronization();

    RefGuard {
      inner: addr_of!(*context),
      _marker: PhantomData,
    }
  }
}

impl<'a, T> Deref for RefGuard<'a, T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    unsafe { (*self.inner).deref() }
  }
}

impl<'a, T> Clone for RefGuard<'a, T>
where
  T: Sync + 'static,
{
  fn clone(&self) -> Self {
    RefGuard {
      inner: self.inner,
      _marker: PhantomData,
    }
  }
}

impl<'a, T> Copy for RefGuard<'a, T> where T: Sync + 'static {}

unsafe impl<'a, T> Send for RefGuard<'a, T> where T: Sync {}
unsafe impl<'a, T> Sync for RefGuard<'a, T> where T: Sync {}

/// LocalKey extension for creating thread-safe pointers to thread-local [`Context`]
#[async_t::async_trait]
pub trait AsyncLocal<T, Ref>
where
  T: 'static + AsContext<Ref>,
  Ref: Sync + 'static,
{
  /// The async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with)
  async fn with_async<F, R, Fut>(&'static self, f: F) -> R
  where
    F: FnOnce(RefGuard<'async_trait, Ref>) -> Fut + Send,
    Fut: Future<Output = R> + Send;

  /// Create a thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`LocalRef`] is within the context of an async runtime
  ///
  /// The well-known way of safely accomplishing these guarantees is to:
  ///
  /// 1) ensure that [`LocalRef`] can only refer to a thread local within the context of the runtime by creating and using only within an async context such as within [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], an async fn or block or within the drop of a pinned [`std::future::Future`] that created [`LocalRef`] prior while pinned and polling.
  ///
  /// 2) ensure that [`LocalRef`] cannot be dereferenced outside of the async runtime context or any thread scoped therein
  ///
  /// 3) use [`pin_project::pinned_drop`](https://docs.rs/pin-project/latest/pin_project/attr.pinned_drop.html) to ensure the safety of dereferencing [`LocalRef`] on the drop impl of a pinned future that created [`LocalRef`] while polling.
  ///
  /// 4) ensure that a move into [`std::thread`] cannot occur or otherwise that [`LocalRef`] cannot be created nor derefenced outside of an async context by constraining use exclusively to within a pinned [`std::future::Future`] being polled or dropped and otherwise using [`RefGuard`] explicitly over any non-`'static` lifetime such as `'async_trait` to allow more flexible usage combined with async traits
  ///
  /// 5) only use [`std::thread::scope`] with validly created [`LocalRef`]
  unsafe fn local_ref(&'static self) -> LocalRef<Ref>;

  /// Create a lifetime-constrained thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`RefGuard`] is within the context of an async runtime
  ///
  /// The well-known way of safely accomplishing these guarantees is to:
  ///
  /// 1) ensure that [`RefGuard`] can only refer to a thread local within the context of the async runtime by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], or an async fn or block or within the drop of a pinned [`std::future::Future`] that created [`RefGuard`] prior while pinned and polling.
  ///
  /// 2) explicitly constrain the lifetime to any non-`'static` lifetime such as`'async_trait`
  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, Ref>;
}

#[async_t::async_trait]
impl<T, Ref> AsyncLocal<T, Ref> for LocalKey<T>
where
  T: 'static + AsContext<Ref>,
  Ref: Sync + 'static,
{
  async fn with_async<F, R, Fut>(&'static self, f: F) -> R
  where
    F: FnOnce(RefGuard<'async_trait, Ref>) -> Fut + Send,
    Fut: Future<Output = R> + Send,
  {
    let local_ref = unsafe { self.guarded_ref() };

    f(local_ref).await
  }

  unsafe fn local_ref(&'static self) -> LocalRef<Ref> {
    self.with(|value| LocalRef::new(value.as_ref()))
  }

  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, Ref> {
    self.with(|value| RefGuard::new(value.as_ref()))
  }
}

#[cfg(all(test, not(loom)))]
mod tests {
  use std::{
    io,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
  };

  use tokio::task::yield_now;

  use super::*;

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn ref_spans_await() {
    let counter = unsafe { COUNTER.local_ref() };
    yield_now().await;
    counter.deref().fetch_add(1, Ordering::Relaxed);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn with_async() {
    COUNTER
      .with_async(|counter| async move {
        yield_now().await;
        counter.fetch_add(1, Ordering::Release);
      })
      .await;
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn bound_to_async_trait_lifetime() {
    struct Counter;
    #[async_t::async_trait]
    trait Countable {
      async fn add_one(ref_guard: RefGuard<'async_trait, AtomicUsize>) -> usize;
    }

    #[async_t::async_trait]
    impl Countable for Counter {
      // Within this context, RefGuard cannot be moved into a blocking thread because of the `'async_trait` lifetime
      async fn add_one(counter: RefGuard<'async_trait, AtomicUsize>) -> usize {
        yield_now().await;
        counter.fetch_add(1, Ordering::Release)
      }
    }

    // here outside of add_one the caller can arbitrarily make this of a `'static` lifetime and hence caution is needed
    let counter = unsafe { COUNTER.guarded_ref() };

    Counter::add_one(counter).await;
  }

  #[test]
  fn tokio_safely_shuts_down() -> io::Result<()> {
    Command::new("cargo")
      .args(["run", "-p", "doomsday-clock"])
      .stdout(Stdio::null())
      .spawn()?
      .wait()?
      .exit_ok()
      .expect("tokio failed to shutdown doomsday-clock");

    Ok(())
  }

  #[test]
  fn async_std_safely_shuts_down() -> io::Result<()> {
    Command::new("cargo")
      .args([
        "run",
        "-p",
        "doomsday-clock",
        "--no-default-features",
        "--features",
        "async-std-runtime",
      ])
      .stdout(Stdio::null())
      .spawn()?
      .wait()?
      .exit_ok()
      .expect("async-std failed to shutdown doomsday-clock");

    Ok(())
  }

  #[ignore]
  #[test]
  fn smol_safely_shuts_down() -> io::Result<()> {
    Command::new("cargo")
      .args([
        "run",
        "-p",
        "doomsday-clock",
        "--no-default-features",
        "--features",
        "smol-runtime",
      ])
      .stdout(Stdio::null())
      .spawn()?
      .wait()?
      .exit_ok()
      .expect("smol to failed to shutdown doomsday-clock");

    Ok(())
  }
}
