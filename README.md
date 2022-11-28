# Async Local
![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

## Thread-safe pointers to thread-locals are possible within an async context

Traditionally the downside of thread-locals has been that usage is constrained to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement thus making usage within an async context limited to calls between await points with no guarantee of reference stability. This makes a lot of sense for a number of reasons: there is no clear way to express the lifetime of a thread, there lifetime's of threads are **never** equivalent and extending the lifetime to 'static can result in dangling pointers during shutdown should references outlive the referenced thread local. Despite all these constraints however, it is yet possible to safely hold pointers to thread locals beyond the standard lifetime of thread locals and across await points by logically constraining usage to exclusively be within an async context. This crate provides safe abstractions such as [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async) as well as the unsafe pointer types and safety considerations for creating narrowly-tailored safe abstractions for using pointers to thread locals soundly in an async context and across await points.

## Runtime Safety

Exclusively during the async runtime shutdown sequence there exists potential for [LocalRef](https://docs.rs/async-local/latest/async_local/struct.LocalRef.html) and [RefGuard](https://docs.rs/async-local/latest/async_local/struct.RefGuard.html) to dereference dangling pointers while dropping unless the runtime itself sequences shutdown in a way that prevents this as described below. This can be avoided by never dereferencing these pointers during drop, or by using a supported runtime such as Tokio or async-std. All single-threaded runtimes are inherently safe to use.

| Runtime      | Shutdown drop safety   | Shutdown Behavior             |
| ------------ | ---------------------- | ----------------------------- |
| Tokio        | Safe; no dangling refs | Drops safely sequenced        |
| async-std    | Safe; no dangling refs | Safe; tasks are not dropped   |
| smol         | Unsafe; deref unsound  | Unsequenced; refs may dangle  |

The Tokio runtime [sequences shutdowns](https://github.com/tokio-rs/tokio/blob/b2f5dbea4703be0c97150b91d3b2c46f29f1a0bf/tokio/src/runtime/runtime.rs#L27-L32) such that worker threads will block until the shutdown operation as completed. This ensures that all tasks will be dropped before any worker thread is dropped, and by virtue of this pointers to TLS variables upholding the pin drop guarantee and held within an async context are valid for all lifetimes except 'static without risk of dangling pointers.

The async-std runtime doesn't drop pending tasks when shutting down and hence avoids the possibility of dangling references occuring during drop altogether.

The Smol runtime should not be used in conjunction with async_local because pointers may dangle during drop when shutting down the runtime and cause undefined behavior.

See [doomsday-clock](https://crates.io/crates/doomsday-clock) for runtime shutdown safety tests.

## Stable Usage

This crate conditionally makes use of the nightly only feature [type_alias_impl_trait](https://rust-lang.github.io/rfcs/2515-type_alias_impl_trait.html) to allow [AsyncLocal::with_async](https://docs.rs/async-local/latest/async_local/trait.AsyncLocal.html#tymethod.with_async) to be unboxed. To compile on `stable` the `boxed` feature flag can be used to downgrade [async_t::async_trait](https://docs.rs/async_t/latest/async_t/attr.async_trait.html) to [async_trait::async_trait](https://docs.rs/async-trait/latest/async_trait).