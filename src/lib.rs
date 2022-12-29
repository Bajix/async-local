#![cfg_attr(not(feature = "boxed"), feature(type_alias_impl_trait))]
#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

assert_cfg!(all(
  not(all(
    feature = "tokio-runtime",
    feature = "async-std-runtime"
  )),
  any(feature = "tokio-runtime", feature = "async-std-runtime",)
));

extern crate self as async_local;

use std::{future::Future, marker::PhantomData, ops::Deref, ptr::addr_of};
#[cfg(not(loom))]
use std::{
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
  thread::LocalKey,
};

#[cfg(feature = "async-std-runtime")]
use async_std::task::{spawn_blocking, JoinHandle};
pub use derive_async_local::AsContext;
#[cfg(loom)]
use loom::{
  sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
  },
  thread::LocalKey,
};
use shutdown_barrier::{guard_thread_shutdown, suspend_until_shutdown};
use static_assertions::assert_cfg;
#[cfg(feature = "tokio-runtime")]
use tokio::task::{spawn_blocking, JoinHandle};

/// A wrapper type used for creating pointers to thread-locals that are valid within an async context
pub struct Context<T: Sync> {
  ref_count: AtomicUsize,
  shutdown_finalized: Mutex<bool>,
  cvar: Condvar,
  inner: T,
}

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// # Usage
  ///
  /// Either wrap a type with Context and assign to a thread-local, or use as an unwrapped field in a struct that derives [AsContext].
  ///
  /// # Example
  ///
  /// ```rust
  /// use std::sync::atomic::AtomicUsize;
  ///
  /// use async_local::Context;
  ///
  /// thread_local! {
  ///   static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  /// }
  /// ```
  pub fn new(inner: T) -> Context<T> {
    Context {
      ref_count: AtomicUsize::new(0),
      shutdown_finalized: Mutex::new(false),
      cvar: Condvar::new(),
      inner,
    }
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
    &self.inner
  }
}

impl<T> Drop for Context<T>
where
  T: Sync,
{
  fn drop(&mut self) {
    if self.ref_count.fetch_add(1, Ordering::Relaxed).ne(&0) {
      let mut shutdown_finalized = self.shutdown_finalized.lock().unwrap();
      while (*shutdown_finalized).eq(&false) {
        shutdown_finalized = self.cvar.wait(shutdown_finalized).unwrap();
      }
    }

    suspend_until_shutdown();
  }
}

struct ContextGuard<T: Sync + 'static>(*const Context<T>);

impl<T> ContextGuard<T>
where
  T: Sync + 'static,
{
  fn new(context: *const Context<T>) -> Self {
    let guard = ContextGuard(context);
    guard.ref_count.fetch_add(1 << 1, Ordering::Relaxed);
    guard
  }
}

impl<T> Deref for ContextGuard<T>
where
  T: Sync,
{
  type Target = Context<T>;
  fn deref(&self) -> &Self::Target {
    unsafe { &*self.0 }
  }
}

impl<T> Drop for ContextGuard<T>
where
  T: Sync + 'static,
{
  fn drop(&mut self) {
    if self.ref_count.fetch_sub(1 << 1, Ordering::Relaxed).eq(&3) {
      *self.shutdown_finalized.lock().unwrap() = true;
      self.cvar.notify_one();
    }
  }
}

unsafe impl<T> Send for ContextGuard<T> where T: Sync {}
unsafe impl<T> Sync for ContextGuard<T> where T: Sync {}

/// A marker trait promising [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is implemented in a way that can't be invalidated
///
/// # Safety
///
/// When assigned to a thread local, the [pin drop guarantee](https://doc.rust-lang.org/std/pin/index.html#drop-guarantee) must be upheld for [`Context<T>`]: it cannot be wrapped in a pointer type nor cell type and it must not be invalidated nor repurposed until when [drop](https://doc.rust-lang.org/std/ops/trait.Drop.html#tymethod.drop) happens as a consequence of the thread dropping.
pub unsafe trait AsContext: AsRef<Context<Self::Target>> {
  type Target: Sync;
}

unsafe impl<T> AsContext for Context<T>
where
  T: Sync,
{
  type Target = T;
}

/// A thread-safe pointer to a thread local [`Context`]
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalRef<T: Sync + 'static>(*const Context<T>);

impl<T> LocalRef<T>
where
  T: Sync + 'static,
{
  unsafe fn new(context: &Context<T>) -> Self {
    guard_thread_shutdown();

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

  /// A wrapper around spawn_blocking that protects [`LocalRef`] for the lifetime of the spawned thread
  ///
  /// Use the `tokio-runtime` feature flag for [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) or `async-std-runtime` for [`async_std::task::spawn_blocking`](https://docs.rs/async-std/latest/async_std/task/fn.spawn_blocking.html)
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "async-std-runtime")))
  )]
  #[cfg(any(feature = "tokio-runtime", feature = "async-std-runtime"))]
  pub fn with_blocking<F, R>(self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(&'a LocalRef<T>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let context_guard = ContextGuard::new(self.0);

    spawn_blocking(move || {
      let result = f(&self);
      drop(context_guard);
      result
    })
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
    guard_thread_shutdown();

    RefGuard {
      inner: addr_of!(*context),
      _marker: PhantomData,
    }
  }

  /// A wrapper around spawn_blocking that protects [`RefGuard`] for the lifetime of the spawned thread
  ///
  /// Use the `tokio-runtime` feature flag for [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) or `async-std-runtime` for [`async_std::task::spawn_blocking`](https://docs.rs/async-std/latest/async_std/task/fn.spawn_blocking.html)
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "async-std-runtime")))
  )]
  #[cfg(any(feature = "tokio-runtime", feature = "async-std-runtime"))]
  pub fn with_blocking<F, R>(self, f: F) -> JoinHandle<R>
  where
    F: for<'b> FnOnce(RefGuard<'b, T>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let context_guard = ContextGuard::new(self.inner);
    let ref_guard = unsafe { std::mem::transmute(self) };

    spawn_blocking(move || {
      let result = f(ref_guard);
      drop(context_guard);
      result
    })
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
pub trait AsyncLocal<T>
where
  T: 'static + AsContext,
{
  /// The async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with)
  async fn with_async<F, R, Fut>(&'static self, f: F) -> R
  where
    F: FnOnce(RefGuard<'async_trait, T::Target>) -> Fut + Send,
    Fut: Future<Output = R> + Send;

  /// A wrapper around spawn_blocking that defers runtime shutdown for the lifetime of the blocking thread
  ///
  /// Use the `tokio-runtime` feature flag for [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) or `async-std-runtime` for [`async_std::task::spawn_blocking`](https://docs.rs/async-std/latest/async_std/task/fn.spawn_blocking.html)
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "async-std-runtime")))
  )]
  #[cfg(any(feature = "tokio-runtime", feature = "async-std-runtime"))]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> R + Send + 'static,
    R: Send + 'static;

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
  unsafe fn local_ref(&'static self) -> LocalRef<T::Target>;

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
  /// 2) Use borrows of any non-`'static` lifetime such as`'async_trait` as a way of constraining the lifetime and preventing [`RefGuard`] from being movable into a blocking thread.
  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, T::Target>;
}

#[async_t::async_trait]
impl<T> AsyncLocal<T> for LocalKey<T>
where
  T: AsContext + 'static,
{
  async fn with_async<F, R, Fut>(&'static self, f: F) -> R
  where
    F: FnOnce(RefGuard<'async_trait, T::Target>) -> Fut + Send,
    Fut: Future<Output = R> + Send,
  {
    let local_ref = unsafe { self.guarded_ref() };

    f(local_ref).await
  }

  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "async-std-runtime")))
  )]
  #[cfg(any(feature = "tokio-runtime", feature = "async-std-runtime"))]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let guarded_ref = unsafe { self.guarded_ref() };
    let context_guard = ContextGuard::new(guarded_ref.inner);

    spawn_blocking(move || {
      let result = f(guarded_ref);
      drop(context_guard);
      result
    })
  }

  unsafe fn local_ref(&'static self) -> LocalRef<T::Target> {
    self.with(|value| LocalRef::new(value.as_ref()))
  }

  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, T::Target> {
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
  async fn with_blocking() {
    COUNTER
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await
      .unwrap();

    let guarded_ref = unsafe { COUNTER.guarded_ref() };

    guarded_ref
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await
      .unwrap();

    let local_ref = unsafe { COUNTER.local_ref() };

    local_ref
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await
      .unwrap();
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn ref_spans_await() {
    let counter = unsafe { COUNTER.local_ref() };
    yield_now().await;
    counter.fetch_add(1, Ordering::Relaxed);
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
      #[allow(clippy::needless_lifetimes)]
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
