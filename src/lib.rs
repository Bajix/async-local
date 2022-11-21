#[cfg(not(loom))]
use std::thread::LocalKey;
use std::{marker::PhantomData, ops::Deref, ptr::addr_of};

#[cfg(loom)]
use loom::thread::LocalKey;
use tokio::{runtime::Handle, task::yield_now};

/// A wrapper around a thread-safe inner type used for creating stable references to thread-locals
/// that are valid for the lifetime of the Tokio runtime and usable within an async context across
/// await points
pub struct Context<T: Sync>(T);

impl<T> Context<T>
where
  T: Sync,
{
  /// Create a new thread-local context
  ///
  /// # Usage
  ///
  /// Either wrap an inner type with Context and assign to a thread-local, or add as an unwrapped field in a struct that implements [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`]>
  ///
  /// # Example
  ///
  /// ```rust
  /// thread_local! {
  ///   static COUNTER: Context<AtomicUsize> = unsafe { Context::new(|| AtomicUsize::new(0)) };
  /// }
  /// ```
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`Context`] is within a thread local variable that upholds the [pin drop guarantee](https://doc.rust-lang.org/std/pin/index.html#drop-guarantee): it cannot be used nor dropped elsewhere; it cannot be wrapped in a pointer type nor cell type; and it must not be invalidated nor repurposed until when [drop](https://doc.rust-lang.org/std/ops/trait.Drop.html#tymethod.drop) happens solely as a consequence of the thread dropping. It does not matter which thread [`Context`] is allocated on, and so it is sound to have publicly visible thread locals using [`Context`] without concern for visibility, but it must be guaranteed that references never exist outside of nor outlive the Tokio runtime by upholding the gaurantees enumerated within [`AsyncLocal`] governing the safe usage of [`LocalRef`] and [`RefGuard`].
  pub unsafe fn new(inner: T) -> Context<T> {
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

/// This ensures that during the Tokio runtime shutdown sequence all tasks are dropped before any
/// thread drops and that dereferencing during drop is always sound.
impl<T> Drop for Context<T>
where
  T: Sync,
{
  fn drop(&mut self) {
    // If a thread local containing [`Context`] is allocated on a blocking thread, there will be no
    // references and there will be no runtime to block on
    while let Ok(handle) = Handle::try_current() {
      // Ensure all tasks are droppped before any [`Context`] is dropped to so that dangling
      // references cannot exist
      handle.block_on(yield_now());
    }
  }
}

/// A thread-safe reference to a thread local [`Context`]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalRef<T: Sync + 'static>(*const Context<T>);

impl<T> LocalRef<T>
where
  T: Sync + 'static,
{
  unsafe fn new(context: &Context<T>) -> Self {
    LocalRef(addr_of!(*context))
  }

  ///
  /// # Safety
  ///
  /// To ensure that it is not possible for [`RefGuard`] to be moved to a thread outside of the
  /// Tokio runtime, this must be constrained to any non-'static lifetime such as 'async_trait
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

unsafe impl<T> Send for LocalRef<T> where T: Sync {}
unsafe impl<T> Sync for LocalRef<T> where T: Sync {}

/// A thread-safe reference to a thread-local [`Context`] constrained by a phantom lifetime
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
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

unsafe impl<'a, T> Send for RefGuard<'a, T> where T: Sync {}
unsafe impl<'a, T> Sync for RefGuard<'a, T> where T: Sync {}

/// LocalKey extension for creating stable thread-safe references to a thread-local [`Context`] that
/// are valid for the lifetime of the Tokio runtime and usable within an async context across await
/// points
pub trait AsyncLocal<T, Ref>
where
  T: 'static + AsRef<Context<Ref>>,
  Ref: Sync,
{
  /// Create a thread-safe reference to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`LocalRef`] is as created from and used within the context of
  /// the Tokio runtime or a thread scoped therein. All behavior must ensure that it is not possible
  /// for [`LocalRef`] to be created within nor dereferenced on a thread outside of the Tokio
  /// runtime.
  ///
  /// The well-known way of safely accomplishing these guarantees is to:
  ///
  /// 1) ensure that [`LocalRef`] can only refer to a thread local within the context of the runtime
  /// by creating and using only within an async context such as [`tokio::spawn`],
  /// [`std::future::Future::poll`], async fn, async block or within the drop of a pinned
  /// [`std::future::Future`] that created [`LocalRef`] prior while pinned and polling.
  ///
  /// 2) limit public usage and ensure that [`LocalRef`] cannot be dereferenced outside of the Tokio
  /// runtime context
  ///
  /// 3) use [`pin_project::pinned_drop`](https://docs.rs/pin-project/latest/pin_project/attr.pinned_drop.html) to ensure the safety of dereferencing [`LocalRef`] on drop impl of a pinned future that created [`LocalRef`] while polling.
  ///
  /// 4) ensure that a move into [`std::thread`] cannot occur or otherwise that [`LocalRef`] cannot
  /// be created nor derefenced outside of an async context by constraining use exclusively to
  /// within a pinned [`std::future::Future`] being polled or dropped and otherwise using
  /// [`RefGuard`] explicitly over any non-`static lifetime such as 'async_trait to allow more
  /// flexible usage combined with async traits
  ///
  /// 5) only use [`std::thread::scope`] with validly created [`LocalRef`]
  unsafe fn local_ref(&'static self) -> LocalRef<Ref>;

  /// Create a lifetime-constrained thread-safe reference to a thread local [`Context`]
  ///
  /// # Safety
  ///
  /// The **only** safe way to use [`RefGuard`] is as created from and used within the context of
  /// the Tokio runtime or a thread scoped therein. All behavior must ensure that it is not possible
  /// for [`RefGuard`] to be created within nor dereferenced on a thread outside of the Tokio
  /// runtime.
  ///
  /// The well-known way of safely accomplishing these guarantees is to:
  ///
  /// 1) ensure that [`RefGuard`] can only refer to a thread local within the context of the Tokio
  /// runtime by creating within an async context such as [`tokio::spawn`],
  /// [`std::future::Future::poll`], or an async fn
  ///
  /// 2) explicitly constrain the lifetime to any non-'static lifetime such as `async_trait
  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, Ref>;
}

impl<T, Ref> AsyncLocal<T, Ref> for LocalKey<T>
where
  T: 'static + AsRef<Context<Ref>>,
  Ref: Sync,
{
  unsafe fn local_ref(&'static self) -> LocalRef<Ref> {
    self.with(|value| LocalRef::new(value.as_ref()))
  }

  unsafe fn guarded_ref<'a>(&'static self) -> RefGuard<'a, Ref> {
    self.with(|value| RefGuard::new(value.as_ref()))
  }
}

#[cfg(all(test, not(loom)))]
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use super::*;

  #[tokio::test(flavor = "multi_thread")]
  async fn ref_spans_await() {
    thread_local! {
        static COUNTER: Context<AtomicUsize> = unsafe { Context::new(AtomicUsize::new(0)) };
    }

    let counter = unsafe { COUNTER.local_ref() };

    for i in 0..100 {
      yield_now().await;
      let count = counter.deref().fetch_add(1, Ordering::Relaxed);
      assert_eq!(i, count);
    }
  }
}
