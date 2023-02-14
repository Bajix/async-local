# Async Local

![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Thread-safe pointers to thread-locals are possible

Traditionally the downside of thead-locals has been that usage is constrainted to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement, the rationale being that anything beyond this is of an indeterminate lifetime. There is however a way around this limitation: by using a barrier to guard against thread local destruction until worker threads shutting down can rendezvous, no tasks will outlive thread local data belonging to any worker thread, and all pointers to thread locals created within an async context and held therein will be of a valid lifetime. Utilizing this barrier mechanism, this crate introduces [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async), the async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with), as well as the unsafe pointer types and safety considerations foundational for using thread local data within an async context and across await points.

## Runtime Configuration

In order for async-local to protect thread local data within an async context, a shutdown barrier must be configured when building the Runtime as to synchronize shutdown across all worker threads. To do so, simply use `#[tokio::main(crate = "async_local", flavor = "multi_thread")]` or `#[tokio::test(crate = "async_local", flavor = "multi_thread")]`


```rust
#[cfg(all(test))]
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use async_local::{AsyncLocal, Context};

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]

  async fn it_increments() {
    COUNTER
      .with_async(|counter| {
        Box::pin(async move {
          counter.fetch_add(1, Ordering::Release);
        })
      })
      .await;
  }
}
```