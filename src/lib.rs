#![cfg_attr(not(feature = "boxed"), feature(type_alias_impl_trait))]
#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

assert_cfg!(not(all(
  feature = "tokio-runtime",
  feature = "async-std-runtime"
)));

extern crate self as async_local;

#[cfg(not(loom))]
use std::thread::LocalKey;
use std::{future::Future, marker::PhantomData, ops::Deref, ptr::addr_of};

#[cfg(feature = "async-std-runtime")]
use async_std::task::{spawn_blocking, JoinHandle};
pub use derive_async_local::AsContext;
#[cfg(loom)]
use loom::thread::LocalKey;
use shutdown_barrier::guard_thread_shutdown;
use static_assertions::assert_cfg;
#[cfg(feature = "tokio-runtime")]
use tokio::task::{spawn_blocking, JoinHandle};

/// A wrapper type used for creating pointers to thread-locals
pub struct Context<T: Sync>(T);

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// # Usage
  ///
  /// Either wrap a type with Context and assign to a thread-local, or use as an unwrapped field in a struct that derives [`AsContext`]
  /// 
  /// # Safety
  /// 
  /// Types that use [`Context`] must not impl [`std::ops::Drop`] because doing so results in the [`thread_local`] macro registering destructor functions that cannot be deferred by blocking with [`std::sync::Condvar`]
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

/// A marker trait promising that [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is implemented in a way that can't be invalidated and that Self doesn't impl [std::ops::Drop]
///
/// # Safety
///
/// Types that implement AsContext cannot impl [std::ops::Drop] because doing so will result in destructor functions being registered by the [`thread_local`] macro that will deallocate memory regardless of whether [`shutdown-barrier::guard_thread_shutdown`](https://docs.rs/shutdown-barrier/latest/shutdown_barrier/fn.guard_thread_shutdown.html) is used to synchronize thread destruction.
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
    spawn_blocking(move || f(&self))
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
    let ref_guard = unsafe { std::mem::transmute(self) };

    spawn_blocking(move || f(ref_guard))
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

  /// A wrapper around spawn_blocking that appropriately constrains the lifetime of [`RefGuard`]
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
  /// The **only** safe way to use [`LocalRef`] is within the context of an async runtime:
  ///
  /// - ensure that [`LocalRef`] refers only to thread locals owned by runtime worker threads by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], an async fn/block or within the [`Drop`] of a pinned [`std::future::Future`] that created [`LocalRef`] prior while pinned and polling.
  ///
  /// - ensure that moves into [`std::thread`] cannot occur unless managed by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) or [`async_std::task::spawn_blocking`](https://docs.rs/async-std/latest/async_std/task/fn.spawn_blocking.html)
  unsafe fn local_ref(&'static self) -> LocalRef<T::Target>;

  /// Create a lifetime-constrained thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`RefGuard`] is within the context of an async runtime:
  ///
  /// - ensure that [`RefGuard`] can only refer to thread locals owned by runtime worker threads by runtime worker threads by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], or an async fn/block or within the [`Drop`] of a pinned [`std::future::Future`] that created [`RefGuard`] prior while pinned and polling.
  ///
  /// - Use borrows of any non-`'static` lifetime such as `'async_trait` as a way of contraining the lifetime as to prevent [`RefGuard`] from being freely movable into blocking threads. Runtime managed threads spawned by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) or [`async_std::task::spawn_blocking`](https://docs.rs/async-std/latest/async_std/task/fn.spawn_blocking.html) can be safely used by re-assigning lifetimes with [`std::mem::transmute`]
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

    spawn_blocking(move || f(guarded_ref))
  }

  unsafe fn local_ref(&'static self) -> LocalRef<T::Target> {
    debug_assert!(
      !std::mem::needs_drop::<T>(),
      "AsyncLocal cannot be used with thread locals types that impl std::ops::Drop"
    );
    self.with(|value| LocalRef::new(value.as_ref()))
  }

  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, T::Target> {
    debug_assert!(
      !std::mem::needs_drop::<T>(),
      "AsyncLocal cannot be used with thread locals types that impl std::ops::Drop"
    );
    self.with(|value| RefGuard::new(value.as_ref()))
  }
}

#[cfg(all(test))]
mod tests {
  #[cfg(not(loom))]
  use std::sync::atomic::AtomicUsize;
  #[cfg(all(
    any(feature = "tokio-runtime", feature = "async-std-runtime"),
    not(loom)
  ))]
  use std::sync::atomic::Ordering;

  #[cfg(feature = "async-std-runtime")]
  use async_std::task::yield_now;
  #[cfg(loom)]
  use loom::{
    sync::atomic::{AtomicUsize, Ordering},
    thread_local,
  };
  #[cfg(feature = "tokio-runtime")]
  use tokio::task::yield_now;

  use super::*;

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[cfg(all(not(loom), feature = "tokio-runtime"))]
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

  #[cfg(all(not(loom), feature = "async-std-runtime"))]
  #[async_std::test]
  async fn with_blocking() {
    COUNTER
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await;

    let guarded_ref = unsafe { COUNTER.guarded_ref() };

    guarded_ref
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await;

    let local_ref = unsafe { COUNTER.local_ref() };

    local_ref
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await;
  }

  #[cfg(loom)]
  #[test]
  fn guard_protects_context() {
    loom::model(|| {
      let counter = Context::new(AtomicUsize::new(0));
      let local_ref = unsafe { LocalRef::new(&counter) };
      let guard = ContextGuard::new(addr_of!(counter));

      loom::thread::spawn(move || {
        let count = local_ref.fetch_add(1, Ordering::Relaxed);
        assert_eq!(count, 0);
        drop(guard);
      });

      drop(counter);
    });
  }

  #[cfg(all(
    not(loom),
    any(feature = "tokio-runtime", feature = "async-std-runtime")
  ))]
  #[cfg_attr(feature = "tokio-runtime", tokio::test(flavor = "multi_thread"))]
  #[cfg_attr(feature = "async-std-runtime", async_std::test)]
  async fn ref_spans_await() {
    let counter = unsafe { COUNTER.local_ref() };
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }

  #[cfg(all(
    not(loom),
    any(feature = "tokio-runtime", feature = "async-std-runtime")
  ))]
  #[cfg_attr(feature = "tokio-runtime", tokio::test(flavor = "multi_thread"))]
  #[cfg_attr(feature = "async-std-runtime", async_std::test)]
  async fn with_async() {
    COUNTER
      .with_async(|counter| async move {
        yield_now().await;
        counter.fetch_add(1, Ordering::Release);
      })
      .await;
  }

  #[cfg(all(
    not(loom),
    any(feature = "tokio-runtime", feature = "async-std-runtime")
  ))]
  #[cfg_attr(feature = "tokio-runtime", tokio::test(flavor = "multi_thread"))]
  #[cfg_attr(feature = "async-std-runtime", async_std::test)]

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
}
