# Async Local

![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Unlocking the potential of thread-locals in an async context

By using a barrier to rendezvous worker threads during runtime shutdown, it can be gauranteed that no task will outlive thread local data belonging to worker threads. With this, pointers to thread locals constrained by invariant lifetimes are guaranteed to be of a valid lifetime suitable for use accross await points. 

## Runtime Configuration (optional)

For best performance, use the Tokio runtime as configured via the [tokio::main](https://docs.rs/tokio/latest/tokio/attr.main.html) or [tokio::test](https://docs.rs/tokio/latest/tokio/attr.test.html) macro with the `crate` attribute set to `async_local` while the `barrier-protected-runtime` feature is enabled on [`async-local`](https://crates.io/crates/async-local). Doing so configures the Tokio runtime with a barrier that rendezvous runtime worker threads during shutdown in a way that ensures tasks never outlive thread local data owned by runtime worker threads and obviates the need for [Box::leak](https://doc.rust-lang.org/std/boxed/struct.Box.html#method.leak) as a means of lifetime extension.

```rust
#[cfg(test)]*
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use async_local::{AsyncLocal, Context};
  use generativity::make_guard;
  use tokio::task::yield_now;

  thread_local! {
      static COUNTER: Context<AtomicUsize> = Context::new(AtomicUsize::new(0));
  }

  #[tokio::test(crate = "async_local", flavor = "multi_thread")]
  async fn it_increments() {
    make_guard!(guard);
    let counter = COUNTER.local_ref(guard);
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }
}
```