#![cfg_attr(test, feature(exit_status_error))]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate self as async_local;

/// A Tokio Runtime builder that configures a barrier to rendezvous worker threads during shutdown to ensure tasks never outlive local data owned by worker threads
#[doc(hidden)]
#[cfg(all(not(loom), feature = "rt"))]
#[path = "runtime.rs"]
pub mod __runtime;

#[cfg(not(feature = "compat"))]
use std::ptr::addr_of;
#[cfg(feature = "compat")]
use std::sync::Arc;
#[cfg(not(loom))]
use std::thread::LocalKey;
use std::{cell::RefCell, ops::Deref};

pub use derive_async_local::{AsContext, main, test};
use generativity::{Guard, Id, make_guard};
#[doc(hidden)]
pub use linkme;
#[cfg(loom)]
use loom::thread::LocalKey;
#[doc(hidden)]
#[cfg(all(not(loom), feature = "rt"))]
pub use tokio::pin;
#[cfg(all(not(loom), feature = "rt"))]
use tokio::task::{JoinHandle, spawn_blocking};

#[derive(PartialEq, Eq, Debug)]
pub(crate) enum BarrierContext {
  Owner,
  /// Tokio Runtime Worker
  RuntimeWorker,
  /// Tokio Pool Worker
  PoolWorker,
}

thread_local! {
  pub(crate) static CONTEXT: RefCell<Option<BarrierContext>> = const { RefCell::new(None) };
}

/// A wrapper type used for creating pointers to thread-locals
pub struct Context<T: Sync + 'static>(
  #[cfg(not(feature = "compat"))] T,
  #[cfg(feature = "compat")] Arc<T>,
);

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// If the `compat` feature flag is enabled, [`Context`] will downgrade to internally using [`std::sync::Arc`] to ensure the validity of `T`
  ///
  /// # Usage
  ///
  /// Either wrap a type with [`Context`] and assign to a thread-local, or use as an unwrapped field in a struct that derives [`AsContext`]
  ///
  /// # Example
  ///
  /// ```rust
  /// use std::sync::atomic::{AtomicUsize, Ordering};
  ///
  /// use async_local::{AsyncLocal, Context};
  /// use generativity::make_guard;
  ///
  /// thread_local! {
  ///   static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  /// }
  ///
  /// #[async_local::main(flavor = "current_thread")]
  /// async fn main() {
  ///   make_guard!(guard);
  ///   let counter = COUNTER.local_ref(guard);
  ///
  ///   let _count = counter
  ///     .with_blocking(|counter| counter.fetch_add(1, Ordering::Relaxed))
  ///     .await
  ///     .unwrap();
  /// }
  /// ```
  pub fn new(inner: T) -> Context<T> {
    #[cfg(not(feature = "compat"))]
    {
      Context(inner)
    }
    #[cfg(feature = "compat")]
    {
      Context(Arc::new(inner))
    }
  }

  /// Construct [`LocalRef`] with an unbounded lifetime.
  ///
  /// # Safety
  ///
  /// This lifetime must be restricted to avoid unsoundness
  pub unsafe fn local_ref<'a>(&self) -> LocalRef<'a, T> {
    unsafe { LocalRef::new(self, Guard::new(Id::new())) }
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
    #[cfg(not(feature = "compat"))]
    {
      &self.0
    }
    #[cfg(feature = "compat")]
    {
      self.0.as_ref()
    }
  }
}

/// A marker trait promising that [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]> is implemented in a way that can't be invalidated
///
/// # Safety
///
/// [`Context`] must not be invalidated as references may exist for the lifetime of the runtime.
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
  #[cfg(not(feature = "compat"))]
  inner: *const T,
  #[cfg(feature = "compat")]
  inner: Arc<T>,
  /// Lifetime carrier
  _brand: Id<'id>,
}

impl<'id, T> LocalRef<'id, T>
where
  T: Sync + 'static,
{
  unsafe fn new(context: &Context<T>, guard: Guard<'id>) -> Self {
    LocalRef {
      #[cfg(not(feature = "compat"))]
      inner: addr_of!(context.0),
      #[cfg(feature = "compat")]
      inner: context.0.clone(),
      _brand: guard.into(),
    }
  }

  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely constrains the lifetime of [`LocalRef`]
  #[cfg(all(not(loom), feature = "rt"))]
  #[cfg_attr(docsrs, doc(cfg(feature = "rt")))]
  pub fn with_blocking<F, R>(self, f: F) -> JoinHandle<R>
  where
    F: for<'a> FnOnce(LocalRef<'a, T>) -> R + Send + 'static,
    R: Send + 'static,
  {
    use std::mem::transmute;

    let local_ref = unsafe { transmute::<LocalRef<'_, T>, LocalRef<'_, T>>(self) };

    spawn_blocking(move || f(local_ref))
  }
}

impl<T> Deref for LocalRef<'_, T>
where
  T: Sync,
{
  type Target = T;
  fn deref(&self) -> &Self::Target {
    #[cfg(not(feature = "compat"))]
    {
      unsafe { &*self.inner }
    }
    #[cfg(feature = "compat")]
    {
      self.inner.deref()
    }
  }
}

impl<T> Clone for LocalRef<'_, T>
where
  T: Sync + 'static,
{
  fn clone(&self) -> Self {
    LocalRef {
      #[cfg(not(feature = "compat"))]
      inner: self.inner,
      #[cfg(feature = "compat")]
      inner: self.inner.clone(),
      _brand: self._brand,
    }
  }
}

unsafe impl<T> Send for LocalRef<'_, T> where T: Sync {}
unsafe impl<T> Sync for LocalRef<'_, T> where T: Sync {}
/// LocalKey extension for creating thread-safe pointers to thread-local [`Context`]
pub trait AsyncLocal<T>
where
  T: AsContext,
{
  /// A wrapper around [`tokio::task::spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) that safely constrains the lifetime of [`LocalRef`]
  #[cfg(all(not(loom), feature = "rt"))]
  #[cfg_attr(docsrs, doc(cfg(feature = "rt")))]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'id> FnOnce(LocalRef<'id, T::Target>) -> R + Send + 'static,
    R: Send + 'static;

  /// Acquire a reference to the value in this TLS key.
  fn with_async<F, R>(&'static self, f: F) -> impl Future<Output = R>
  where
    F: for<'a> AsyncFnMut(LocalRef<'a, T::Target>) -> R;

  /// Create a pointer to a thread local [`Context`] using a trusted lifetime carrier.
  ///
  /// # Usage
  ///
  /// Use [`generativity::make_guard`] to generate a unique [`invariant`](https://doc.rust-lang.org/nomicon/subtyping.html#variance) lifetime brand
  ///
  /// # Panic
  ///
  /// [`LocalRef`] must be created within the async context of the runtime.
  fn local_ref<'id>(&'static self, guard: Guard<'id>) -> LocalRef<'id, T::Target>;
}

impl<T> AsyncLocal<T> for LocalKey<T>
where
  T: AsContext,
{
  #[cfg(all(not(loom), feature = "rt"))]
  #[cfg_attr(docsrs, doc(cfg(feature = "rt")))]
  fn with_blocking<F, R>(&'static self, f: F) -> JoinHandle<R>
  where
    F: for<'id> FnOnce(LocalRef<'id, T::Target>) -> R + Send + 'static,
    R: Send + 'static,
  {
    let guard = unsafe { Guard::new(Id::new()) };
    let local_ref = self.local_ref(guard);
    spawn_blocking(move || f(local_ref))
  }

  async fn with_async<F, R>(&'static self, mut f: F) -> R
  where
    F: for<'a> AsyncFnMut(LocalRef<'a, T::Target>) -> R,
  {
    make_guard!(guard);
    let local_ref = self.local_ref(guard);
    f(local_ref).await
  }

  #[track_caller]
  #[inline(always)]
  fn local_ref<'id>(&'static self, guard: Guard<'id>) -> LocalRef<'id, T::Target> {
    #[cfg(not(feature = "compat"))]
    {
      if CONTEXT
        .with(|context| matches!(&*context.borrow(), None | Some(BarrierContext::PoolWorker)))
      {
        panic!(
          "LocalRef can only be created within the async context of a Tokio Runtime configured by `#[async_local::main]` or `#[async_local::test]`"
        );
      }
    }

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

  #[async_local::test]
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

  #[async_local::test]
  async fn ref_spans_await() {
    make_guard!(guard);
    let counter = COUNTER.local_ref(guard);
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }

  #[async_local::test]
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
