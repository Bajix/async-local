# Shutdown Barrier
![License](https://img.shields.io/badge/license-MIT-green.svg)
[![Cargo](https://img.shields.io/crates/v/shutdown-barrier.svg)](https://crates.io/crates/shutdown-barrier)
[![Documentation](https://docs.rs/shutdown-barrier/badge.svg)](https://docs.rs/shutdown-barrier)

By utilizing a barrier to synchronize runtime threads during the destruction of thread local data, no tasks will outlive thread local data belonging to to any worker thread, and all pointers to thread locals created within an async context and held therein will be of a valid lifetime.

See [async-local](https://crates.io/crates/async-local)