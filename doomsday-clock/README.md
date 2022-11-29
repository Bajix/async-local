# Doomsday Clock

The validity of [`async-local`](https://crates.io/crates/async-local) is predicated upon the guarantee that for a given async runtime, tasks either can't be stolen or shutdown behavior assures no task be polled nor dropped after any worker thread dropped. So long as these invariants are upheld, it is sound to use raw pointers to TLS variables that uphold the [pin drop guarantee](https://doc.rust-lang.org/std/pin/index.html#drop-guarantee) so long as these pointers are never created nor dereferenced outside of an async context. Without these guarantees, dangling references will result in undefined behavior intermittently during the course of the runtime being shutdown. Doomsday Clock is an async runtime shutdown test specifically designed to panic should shutdown not be sequenced in a way that will avoid potential dangling references using the techniques outlined within [`async-local`](https://crates.io/crates/async-local).