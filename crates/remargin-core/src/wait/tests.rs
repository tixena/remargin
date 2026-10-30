//! Tests for [`crate::wait`].

use core::future::{poll_fn, ready};
use core::task::Poll;
use std::thread;

use super::Wait as _;

#[test]
fn returns_the_output_of_a_ready_future() {
    assert_eq!(ready(7_i32).wait(), 7_i32);
}

#[test]
fn parks_until_another_thread_wakes_the_future() {
    let mut polls = 0_i32;
    let output = poll_fn(|context| {
        polls += 1_i32;
        if polls == 1_i32 {
            let waker = context.waker().clone();
            thread::spawn(move || waker.wake());
            return Poll::Pending;
        }
        Poll::Ready(polls)
    })
    .wait();
    assert_eq!(output, 2_i32);
}
