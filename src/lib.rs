#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate self as async_local;

/// A Tokio Runtime builder configured with a barrier that rendezvous worker threads during shutdown as to ensure tasks never outlive local data owned by worker threads
#[cfg(all(not(loom), feature = "tokio-runtime"))]
#[cfg_attr(
  docsrs,
  doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
)]
pub mod runtime;

#[cfg(not(loom))]
use std::thread::LocalKey;
use std::{
  future::Future, hint::unreachable_unchecked, marker::PhantomData, ops::Deref, pin::Pin,
  ptr::addr_of, task::Poll,
};

pub use derive_async_local::AsContext;
#[cfg(loom)]
use loom::thread::LocalKey;
use pin_project::pin_project;
#[doc(hidden)]
#[cfg(all(not(loom), feature = "tokio-runtime"))]
pub use tokio::pin;
#[cfg(all(not(loom), feature = "tokio-runtime"))]
use tokio::task::{spawn_blocking, JoinHandle};

/// A wrapper type used for creating pointers to thread-locals
#[cfg(not(feature = "barrier-protected-runtime"))]
pub struct Context<T: Sync + 'static>(&'static T);

/// A wrapper type used for creating pointers to thread-locals
#[cfg(feature = "barrier-protected-runtime")]
pub struct Context<T: Sync + 'static>(T);

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// If the `barrier-protected-runtime` feature flag isn't enabled, [`Context`] will use [`Box::leak`] to avoid `T` ever being deallocated instead of relying on the provided barrier-protected Tokio runtime to ensure tasks never outlive thread local data owned by worker threads. This provides compatibility at the cost of performance for whenever it's not well-known that the async runtime used is the Tokio [`runtime`] configured by this crate.
  ///
  /// # Usage
  ///
  /// Either wrap a type with [`Context`] and assign to a thread-local, or use as an unwrapped field in a struct that derives [`AsContext`]
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
  #[cfg(feature = "barrier-protected-runtime")]
  pub fn new(inner: T) -> Context<T> {
    Context(inner)
  }

  #[cfg(not(feature = "barrier-protected-runtime"))]
  pub fn new(inner: T) -> Context<T> {
    Context(Box::leak(Box::new(inner)))
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

#[cfg(not(feature = "barrier-protected-runtime"))]
impl<T> Deref for Context<T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    self.0
  }
}

#[cfg(feature = "barrier-protected-runtime")]
impl<T> Deref for Context<T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

/// A marker trait promising that [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is implemented in a way that can't be invalidated
///
/// # Safety
///
/// Context must not be a type that can be invalidated as references may exist for the lifetime of the runtime.
pub unsafe trait AsContext: AsRef<Context<Self::Target>> {
  type Target: Sync + 'static;
}

unsafe impl<T> AsContext for Context<T>
where
  T: Sync,
{
  type Target = T;
}

/// A thread-safe pointer to a thread local [`Context`]
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalRef<T: Sync + 'static>(*const T);

impl<T> LocalRef<T>
where
  T: Sync + 'static,
{
  #[cfg(not(feature = "barrier-protected-runtime"))]
  unsafe fn new(context: &Context<T>) -> Self {
    LocalRef(addr_of!(*context.0))
  }

  #[cfg(feature = "barrier-protected-runtime")]
  unsafe fn new(context: &Context<T>) -> Self {
    LocalRef(addr_of!(context.0))
  }

  /// # Safety
  ///
  /// To ensure that it is not possible for [`RefGuard`] to be moved to a thread outside of the async runtime, this must be constrained to a non-`'static` lifetime
  pub unsafe fn guarded_ref<'a>(&self) -> RefGuard<'a, T> {
    RefGuard {
      inner: self.0,
      _marker: PhantomData,
    }
  }

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely extends and constrains the lifetime of [`LocalRef`]
  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
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
    unsafe { &*self.0 }
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
  inner: *const T,
  _marker: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<'a, T> RefGuard<'a, T>
where
  T: Sync + 'static,
{
  #[cfg(not(feature = "barrier-protected-runtime"))]
  unsafe fn new(context: &Context<T>) -> Self {
    RefGuard {
      inner: addr_of!(*context.0),
      _marker: PhantomData,
    }
  }

  #[cfg(feature = "barrier-protected-runtime")]
  unsafe fn new(context: &Context<T>) -> Self {
    RefGuard {
      inner: addr_of!(context.0),
      _marker: PhantomData,
    }
  }

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely extends and constrains the lifetime of [`RefGuard`]
  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
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
    unsafe { &*self.inner }
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

/// A Future with a reference to a thread local [`Context`] that's usable across await points
#[pin_project(project = WithLocalProj, project_replace = WithLocalOwn)]
pub enum WithLocal<T, F, R>
where
  T: AsContext + 'static,
  F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
{
  Uninitialized { f: F, key: &'static LocalKey<T> },
  Initializing { _marker: PhantomData<R> },
  Pending(#[pin] Pin<Box<dyn Future<Output = R> + Send>>),
}

impl<T, F, R> WithLocal<T, F, R>
where
  T: AsContext + 'static,
  F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
{
  pub fn new(f: F, key: &'static LocalKey<T>) -> Self {
    WithLocal::Uninitialized { f, key }
  }
}

impl<T, F, R> Future for WithLocal<T, F, R>
where
  T: AsContext,
  F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
{
  type Output = R;

  fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
    let inner = match self.as_mut().project() {
      WithLocalProj::Uninitialized { f: _, key: _ } => {
        match self.as_mut().project_replace(WithLocal::Initializing {
          _marker: PhantomData,
        }) {
          WithLocalOwn::Uninitialized { f, key } => {
            let guarded_ref = unsafe { key.guarded_ref() };
            self
              .as_mut()
              .project_replace(WithLocal::Pending(f(guarded_ref)));
            match self.as_mut().project() {
              WithLocalProj::Pending(inner) => inner,
              _ => unsafe {
                unreachable_unchecked();
              },
            }
          }
          _ => unsafe {
            unreachable_unchecked();
          },
        }
      }
      WithLocalProj::Initializing { _marker } => unsafe {
        unreachable_unchecked();
      },
      WithLocalProj::Pending(inner) => inner,
    };

    inner.poll(cx)
  }
}

/// LocalKey extension for creating thread-safe pointers to thread-local [`Context`]
pub trait AsyncLocal<T>
where
  T: AsContext,
{
  /// The async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with)
  fn with_async<F, R>(&'static self, f: F) -> WithLocal<T, F, R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>;

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely extends and constrains the lifetime of [`RefGuard`]
  #[cfg(all(not(loom), any(feature = "tokio-runtime")))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> R + Send + 'static,
    R: Send + 'static;

  /// Create a thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`LocalRef`] is within the context of a barrier-protected Tokio runtime:
  ///
  /// - ensure that [`LocalRef`] refers only to thread locals owned by runtime worker threads by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], an async fn/block or within the [`Drop`] of a pinned [`std::future::Future`] that created [`LocalRef`] prior while pinned and polling.
  ///
  /// - ensure that moves into [`std::thread`] cannot occur unless managed by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) and constrained therein
  unsafe fn local_ref(&'static self) -> LocalRef<T::Target>;

  /// Create a lifetime-constrained thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`RefGuard`] is within the context of a barrier-protected Tokio runtime:
  ///
  /// - ensure that [`RefGuard`] can only refer to thread locals owned by runtime worker threads by runtime worker threads by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], or an async fn/block or within the [`Drop`] of a pinned [`std::future::Future`] that created [`RefGuard`] prior while pinned and polling.
  ///
  /// - constrain to a non-`'static` lifetime as to prevent [`RefGuard`] from being freely movable into blocking threads. Runtime managed threads spawned by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) can be safely used by re-assigning lifetimes with [`std::mem::transmute`]
  ///
  /// - closures can only adequately constrain the lifetime by using Higher-Rank Trait Bounds (see [HRTBs](https://doc.rust-lang.org/nomicon/hrtb.html)); futures returned must be boxed, pinned and generic over an arbitrary lifetime
  ///
  /// ```rust
  /// F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>
  /// ```
  ///
  /// - for async trait fns using [`async_t`](https://crates.io/crates/async_t) or [`async_trait`](https://crates.io/crates/async-trait), the lifetime `'async_trait` is suitable to constrain [`RefGuard`] from escaping to `'static`
  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, T::Target>;
}

impl<T> AsyncLocal<T> for LocalKey<T>
where
  T: AsContext,
{
  fn with_async<F, R>(&'static self, f: F) -> WithLocal<T, F, R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
  {
    WithLocal::new(f, self)
  }

  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(RefGuard<'a, T::Target>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let guarded_ref = unsafe { self.guarded_ref() };

    spawn_blocking(move || f(guarded_ref))
  }

  unsafe fn local_ref(&'static self) -> LocalRef<T::Target> {
    self.with(|value| LocalRef::new(value.as_ref()))
  }

  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, T::Target> {
    self.with(|value| RefGuard::new(value.as_ref()))
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use tokio::task::yield_now;

  use super::*;

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
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

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn ref_spans_await() {
    let counter = unsafe { COUNTER.local_ref() };
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn with_async() {
    COUNTER
      .with_async(|counter| {
        Box::pin(async move {
          yield_now().await;
          counter.fetch_add(1, Ordering::Release);
        })
      })
      .await;
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn bound_to_async_trait_lifetime() {
    struct Counter;
    #[async_trait::async_trait]
    trait Countable {
      #[allow(clippy::needless_lifetimes)]
      async fn add_one(ref_guard: RefGuard<'async_trait, AtomicUsize>) -> usize;
    }

    #[async_trait::async_trait]
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
