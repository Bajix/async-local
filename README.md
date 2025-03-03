# Async Local

![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Unlocking the potential of thread-locals in an async context

This crate enables references to thread locals to be used in an async context and across await points or within blocking threads managed by the Tokio runtime

## How it works

By configuring Tokio with a barrier to rendezvous worker threads during shutdown, it can be gauranteed that no task will outlive thread local data belonging to worker threads. With this, pointers to thread locals constrained by invariant lifetimes are guaranteed to be of a valid lifetime suitable for use accross await points.

## Runtime Configuration (optional)

In order to enable the optimization this crate provides, use the Tokio runtime as configured via the [tokio::main](https://docs.rs/tokio/latest/tokio/attr.main.html) or [tokio::test](https://docs.rs/tokio/latest/tokio/attr.test.html) macro with `crate` set as `async_local` while the `barrier-protected-runtime` feature flag is enabled.

## Example usage

```rust
#[cfg(test)]
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