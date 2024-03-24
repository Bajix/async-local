#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate self as async_local;

/// A Tokio Runtime builder configured with a barrier that rendezvous worker threads during shutdown as to ensure tasks never outlive local data owned by worker threads
#[cfg(all(not(loom), feature = "tokio-runtime"))]
#[cfg_attr(
  docsrs,
  doc(cfg(any(feature = "barrier-protected-runtime")))
)]
pub mod runtime;

#[cfg(not(loom))]
use std::thread::LocalKey;
use std::{ops::Deref, ptr::addr_of};

pub use derive_async_local::AsContext;
use generativity::{Guard, Id};
#[cfg(loom)]
use loom::thread::LocalKey;
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
  /// If the `barrier-protected-runtime` feature flag isn't enabled, [`Context`] will use [`Box::leak`] to avoid `T` ever being deallocated instead of relying on the provided barrier-protected Tokio runtime to ensure tasks never outlive thread local data owned by worker threads. This provides soundness at the cost of performance for whenever it's not well-known that the async runtime used is the Tokio [`runtime`] configured by this crate.
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

  /// Construct [`LocalRef`] with an unbounded lifetime.
  ///
  /// # Safety
  ///
  /// This lifetime must be restricted to avoid unsoundness
  pub unsafe fn local_ref<'a>(&self) -> LocalRef<'a, T> {
    LocalRef::new(self, Guard::new(Id::new()))
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
/// [`Context`] must not be a type that can be invalidated as references may exist for the lifetime of the runtime.
pub unsafe trait AsContext: AsRef<Context<Self::Target>> {
  type Target: Sync + 'static;
}

unsafe impl<T> AsContext for Context<T>
where
  T: Sync,
{
  type Target = T;
}

/// A thread-safe pointer to a thread-local [`Context`] constrained by a "[generative](https://crates.io/crates/generativity)" lifetime brand that is [invariant](https://doc.rust-lang.org/nomicon/subtyping.html#variance) over the lifetime parameter and cannot be coerced into `'static`
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalRef<'id, T: Sync + 'static> {
  inner: *const T,
  /// Lifetime carrier
  _brand: Id<'id>,
}

impl<'id, T> LocalRef<'id, T>
where
  T: Sync + 'static,
{
  #[cfg(not(feature = "barrier-protected-runtime"))]
  unsafe fn new(context: &Context<T>, guard: Guard<'id>) -> Self {
    LocalRef {
      inner: addr_of!(*context.0),
      _brand: guard.into(),
    }
  }

  #[cfg(feature = "barrier-protected-runtime")]
  unsafe fn new(context: &Context<T>, guard: Guard<'id>) -> Self {
    LocalRef {
      inner: addr_of!(context.0),
      _brand: guard.into(),
    }
  }

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely constrains the lifetime of [`LocalRef`]
  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
  pub fn with_blocking<F, R>(self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(LocalRef<'a, T>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let local_ref = unsafe { std::mem::transmute(self) };

    spawn_blocking(move || f(local_ref))
  }
}

impl<'id, T> Deref for LocalRef<'id, T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    unsafe { &*self.inner }
  }
}

impl<'id, T> Clone for LocalRef<'id, T>
where
  T: Sync + 'static,
{
  fn clone(&self) -> Self {
    *self
  }
}

impl<'id, T> Copy for LocalRef<'id, T> where T: Sync + 'static {}

unsafe impl<'id, T> Send for LocalRef<'id, T> where T: Sync {}
unsafe impl<'id, T> Sync for LocalRef<'id, T> where T: Sync {}
/// LocalKey extension for creating thread-safe pointers to thread-local [`Context`]
pub trait AsyncLocal<T>
where
  T: AsContext,
{
  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely constrains the lifetime of [`LocalRef`]
  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'id> FnOnce(LocalRef<'id, T::Target>) -> R + Send + 'static,
    R: Send + 'static;

  /// Create a pointer to a thread local [`Context`] using a trusted lifetime carrier.
  ///
  /// # Usage
  ///
  /// Use [`generativity::make_guard`] to generate a unique [`invariant`](https://doc.rust-lang.org/nomicon/subtyping.html#variance) lifetime brand
  fn local_ref<'id>(&'static self, guard: Guard<'id>) -> LocalRef<'id, T::Target>;
}

impl<T> AsyncLocal<T> for LocalKey<T>
where
  T: AsContext,
{
  #[cfg(all(not(loom), feature = "tokio-runtime"))]
  #[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "tokio-runtime", feature = "barrier-protected-runtime")))
  )]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'id> FnOnce(LocalRef<'id, T::Target>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let guard = unsafe { Guard::new(Id::new()) };
    let local_ref = self.local_ref(guard);
    spawn_blocking(move || f(local_ref))
  }

  fn local_ref<'id>(&'static self, guard: Guard<'id>) -> LocalRef<'id, T::Target> {
    self.with(|value| unsafe { LocalRef::new(value.as_ref(), guard) })
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use generativity::make_guard;
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

    make_guard!(guard);
    let local_ref = COUNTER.local_ref(guard);

    local_ref
      .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
      .await
      .unwrap();
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn ref_spans_await() {
    make_guard!(guard);
    let counter = COUNTER.local_ref(guard);
    yield_now().await;
    async {}.await;
    counter.fetch_add(1, Ordering::SeqCst);
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn with_async_trait() {
    struct Counter;

    trait Countable {
      async fn add_one(ref_guard: LocalRef<'_, AtomicUsize>) -> usize;
    }

    impl Countable for Counter {
      async fn add_one(counter: LocalRef<'_, AtomicUsize>) -> usize {
        yield_now().await;
        counter.fetch_add(1, Ordering::Release)
      }
    }

    make_guard!(guard);
    let counter = COUNTER.local_ref(guard);

    Counter::add_one(counter).await;
  }
}
