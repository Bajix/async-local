# Async Local
![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Thread-safe pointers to thread-locals are possible within an async context

Traditionally the downside of thead-locals has been that usage is constrainted to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement, the rationale being that anything beyond this is of an indeterminate lifetime. There is however a way around this limitation: by using a barrier to guard against thread local destruction until worker threads shutting down can rendezvous, no tasks will outlive thread local data belonging to any worker thread, and all pointers to thread locals created within an async context and held therein will be of a valid lifetime. Utilizing this barrier mechanism, this crate introduces [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async), the async counterpart of [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with), as well as the unsafe pointer types and safety considerations foundational for using thread local data within an async context and across await points.
## Runtime Safety

The only requirement for async runtimes to be compatible with [async-local](https://crates.io/crates/async-local) is that pending async tasks aren't dropped by thread local destructors.

| Runtime   | Support       | Shutdown Behavior                      |
| --------- | ------------- | -------------------------------------- |
| Tokio     | Supported     | Tasks dropped during shutdown sequence |
| async-std | Supported     | Tasks forgotten                        |
| smol      | Not Supported | Tasks dropped by TLS destructors       |

See [doomsday-clock](https://crates.io/crates/doomsday-clock) for runtime shutdown safety tests.

## Stable Usage

This crate conditionally makes use of the nightly only feature [type_alias_impl_trait](https://rust-lang.github.io/rfcs/2515-type_alias_impl_trait.html) to allow [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async) to be unboxed. To compile on `stable` the `boxed` feature flag can be used to downgrade [async_t::async_trait](https://docs.rs/async_t/latest/async_t/attr.async_trait.html) to [async_trait::async_trait](https://docs.rs/async-trait/latest/async_trait).
