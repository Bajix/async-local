# Async Local
![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/async-local.svg)](https://crates.io/crates/async-local)
[![Documentation](https://docs.rs/async-local/badge.svg)](https://docs.rs/async-local)

Traditionally the downside of thread-locals has been that usage is constrained to the [LocalKey::with](https://doc.rust-lang.org/std/thread/struct.LocalKey.html#method.with) closure with no lifetime escapement thus making usage within an async context limited to calls between await points with no guarantee of reference stability. This makes a lot of sense for a number of reasons: there is no clear way to express the lifetime of a thread, the lifetime's of threads are **never** equivalent, extending the lifetime to 'static can result in dangling pointers during shutdown should references be moved onto bocking threads that outlive the referenced thread local, and because during runtime shutdown references to thread locals from other runtime threads may dangle on drop. Despite all these constraints however, there yet exists ways to make it possible to hold references to thread locals beyond the standard lifetime of thread locals and across await points by creating safe abstractions that logically constrain usage to exclusively be within an async context by tying to non-'static lifetimes such as 'async_trait or that of a [Future](https://doc.rust-lang.org/stable/std/future/trait.Future.html) being polled. This crate provides the unsafe building blocks and safety considerations for creating narrowly-tailored safe abstractions capable of using stable references to thread locals soundly in an async context and across await points.