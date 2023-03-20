# Async Local

![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Thread-safe pointers to thread-locals are possible

Traditionally the downside of thead-locals has been that usage is constrainted to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement, the rationale being that anything beyond this is of an indeterminate lifetime. There is however a way around this limitation: by using a barrier to rendezvous worker threads during runtime shutdown, no tasks will outlive thread local data belonging to any worker thread, and all pointers to thread locals created within an async context and held therein will be of a valid lifetime. Utilizing this barrier mechanism, this crate introduces [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async), the async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with), as well as the unsafe pointer types and safety considerations foundational for using thread local data within an async context.

## Runtime Configuration (optional)

For best performance, use the Tokio runtime as configured via the [tokio::main](https://docs.rs/tokio/latest/tokio/attr.main.html) or [tokio::test](https://docs.rs/tokio/latest/tokio/attr.test.html) macro with the `crate` attribute set to `async_local` while the `barrier-protected-runtime` feature is enabled on [`async-local`](https://crates.io/crates/async-local). Doing so configures the Tokio runtime with a barrier that rendezvous runtime worker threads during shutdown in a way that ensures tasks never outlive thread local data owned by runtime worker threads and obviates the need for [Box::leak](https://doc.rust-lang.org/std/boxed/struct.Box.html#method.leak) as a means of lifetime extension.

```rust
#[cfg(test)]*
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