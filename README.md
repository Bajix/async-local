# Async Local

![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Unlocking the potential of thread-locals in an async context

This crate enables references to thread locals to be used in an async context across await points or within blocking threads managed by the Tokio runtime

## How it works

By configuring Tokio with a barrier to rendezvous worker threads during shutdown, it can be gauranteed that no task will outlive thread local data belonging to worker threads. With this, pointers to thread locals constrained by invariant lifetimes are guaranteed to be of a valid lifetime suitable for use accross await points.

## Runtime Configuration

The optimization that this crate provides require that the [async_local::main](https://docs.rs/async-local/latest/async_local/attr.main.html) or [async_local::test](https://docs.rs/async-local/latest/async_local/attr.test.html) macro be used to configure the Tokio runtime. This is enforced by a pre-main check that asserts [async_local::main](https://docs.rs/async-local/latest/async_local/attr.main.html) has been used.

## Tests

Running tests requires the `compat` feature. To enable the feature just for your development environment, add the crate with this feature to your `dev-dependencies` in `Cargo.toml`:

```toml
[dependencies]
async-local = "6.0.1"

[dev-dependencies]
async-local = { version = "6.0.1", features = ["compat"] }
```

## Compatibility Mode

Enabling the `compat` feature flag will allow this crate to be used with any runtime configuration by disabling the performance optimization this crate provides and instead internally using `std::sync::Arc`

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

  #[async_local::test]
  async fn it_increments() {
    make_guard!(guard);
    let counter = COUNTER.local_ref(guard);
    yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
  }
}
```
