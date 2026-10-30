//! Runs the futures of `os_shim::System` to the end from remargin's synchronous code.
//!
//! The futures of `RealSystem` over the disk and of `MemorySystem` are ready on their first poll,
//! so a call holds its thread for as long as the file operation takes and no runtime is involved.

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::thread::{self, Thread};

/// Wakes the thread parked in [`Wait::wait`].
struct ParkedThread(Thread);

impl Wake for ParkedThread {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// A future that the calling thread can run to its end.
pub trait Wait: Future + Sized {
    /// Polls the future on this thread, parking between polls, and returns its output. It starts
    /// no runtime, so it is safe inside one, where a future that does wait blocks that thread.
    fn wait(self) -> Self::Output;
}

impl<Fut: Future> Wait for Fut {
    #[inline]
    fn wait(self) -> Fut::Output {
        let waker = Waker::from(Arc::new(ParkedThread(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(self);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            thread::park();
        }
    }
}

#[cfg(test)]
mod tests;
