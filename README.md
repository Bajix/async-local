# Async Local
![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Thread-safe pointers to thread-locals are possible within an async context

Traditionally the downside of thread-locals has been that usage is constrained to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement thus preventing immutable references and limiting usability in async context. This makes a lot of sense: the lifetime of unscoped threads is indeterminate, and references may dangle should threads outlive what they reference. There is however an escape hatch to these limitations: by utilizing a synchronization barrier across all runtime worker threads as a guard to protect the destruction of thread local data, runtime shutdowns can be sequenced in a way that guarantees no task be dropped after this barrier, and in doing so it can be ensured that so long as the [pin drop guarantee](https://doc.rust-lang.org/std/pin/index.html#drop-guarantee) is upheld, pointers to thread local data created within an async context and held therein will never dangle. This crate provides safe abstractions such as [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async), the async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with), as well as the unsafe pointer types and safety considerations for creating narrowly-tailored safe abstractions for using pointers to thread locals soundly in an async context and across await points.

## Runtime Safety

The only requirement for async runtimes to be compatible with [async-local](https://crates.io/crates/async-local) is that pending async tasks aren't dropped by thread local destructors. The Tokio runtime ensures this by [sequencing shutdowns](https://github.com/tokio-rs/tokio/blob/b2f5dbea4703be0c97150b91d3b2c46f29f1a0bf/tokio/src/runtime/runtime.rs#L27-L32).

| Runtime   | Support       | Shutdown Behavior                      |
| --------- | ------------- | -------------------------------------- |
| Tokio     | Supported     | Tasks dropped during shutdown sequence |
| async-std | Supported     | Tasks forgotten                        |
| smol      | Not Supported | Tasks dropped by TLS destructors       |

See [doomsday-clock](https://crates.io/crates/doomsday-clock) for runtime shutdown safety tests.

## Stable Usage

This crate conditionally makes use of the nightly only feature [type_alias_impl_trait](https://rust-lang.github.io/rfcs/2515-type_alias_impl_trait.html) to allow [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async) to be unboxed. To compile on `stable` the `boxed` feature flag can be used to downgrade [async_t::async_trait](https://docs.rs/async_t/latest/async_t/attr.async_trait.html) to [async_trait::async_trait](https://docs.rs/async-trait/latest/async_trait).