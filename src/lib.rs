#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

/// A barrier protected Tokio Runtime
pub mod runtime;

extern crate self as async_local;

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
pub use tokio::pin;
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

/// A marker trait promising that [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is implemented in a way that can't be invalidated
///
/// # Safety
///
/// Context must not be a type that can be invalidated as references may exist for the lifetime of the runtime.
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
    RefGuard {
      inner: addr_of!(*context),
      _marker: PhantomData,
    }
  }

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that protects [`RefGuard`] for the lifetime of the spawned thread
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

  /// A wrapper around spawn_blocking that appropriately constrains the lifetime of [`RefGuard`]
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
  /// - ensure that moves into [`std::thread`] cannot occur unless managed by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
  unsafe fn local_ref(&'static self) -> LocalRef<T::Target>;

  /// Create a lifetime-constrained thread-safe pointer to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`RefGuard`] is within the context of a barrier-protected Tokio runtime:
  ///
  /// - ensure that [`RefGuard`] can only refer to thread locals owned by runtime worker threads by runtime worker threads by creating within an async context such as [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), [`std::future::Future::poll`], or an async fn/block or within the [`Drop`] of a pinned [`std::future::Future`] that created [`RefGuard`] prior while pinned and polling.
  ///
  /// - Contrain to a non-'static lifetime as to prevent [`RefGuard`] from being freely movable into blocking threads. Runtime managed threads spawned by [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) can be safely used by re-assigning lifetimes with [`std::mem::transmute`]
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
    WithLocal::Uninitialized { f, key: self }
  }

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

#[cfg(all(test))]
mod tests {
  #[cfg(not(loom))]
  use std::sync::atomic::{AtomicUsize, Ordering};

  #[cfg(loom)]
  use loom::{
    sync::atomic::{AtomicUsize, Ordering},
    thread_local,
  };
  #[cfg(not(loom))]
  use tokio::task::yield_now;

  use super::*;

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[cfg(not(loom))]
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

  #[cfg(not(loom))]
  #[tokio::test(crate = "async_local", flavor = "multi_thread")]

  async fn ref_spans_await() {
    let counter = unsafe { COUNTER.local_ref() };
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }

  #[cfg(not(loom))]
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

  #[cfg(not(loom))]
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
