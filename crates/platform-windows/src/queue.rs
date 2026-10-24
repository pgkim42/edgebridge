//! Bounded lock-free single-producer/single-consumer ring. Allocation happens once in `channel`;
//! `Producer::push` and `Consumer::pop` never allocate, lock, or block.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(align(64))]
struct Padded(AtomicUsize);

struct Ring<T> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    /// Next slot the consumer reads; written only by the consumer.
    head: Padded,
    /// Next slot the producer writes; written only by the producer.
    tail: Padded,
}

// SAFETY: slots are handed over with release/acquire on head/tail; each slot is accessed by exactly
// one side at a time (producer owns [tail, head+cap), consumer owns [head, tail)). `T: Copy` means
// no drop glue for unread slots.
unsafe impl<T: Copy + Send> Send for Ring<T> {}
unsafe impl<T: Copy + Send> Sync for Ring<T> {}

pub struct Producer<T> {
    ring: Arc<Ring<T>>,
}

pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// Creates a queue holding at least `capacity` items (rounded up to a power of two, minimum 2).
pub fn channel<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(2).next_power_of_two();
    let buf: Box<[UnsafeCell<MaybeUninit<T>>]> = (0..cap)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect();
    let ring = Arc::new(Ring {
        buf,
        mask: cap - 1,
        head: Padded(AtomicUsize::new(0)),
        tail: Padded(AtomicUsize::new(0)),
    });
    (
        Producer {
            ring: Arc::clone(&ring),
        },
        Consumer { ring },
    )
}

impl<T: Copy + Send> Producer<T> {
    pub fn capacity(&self) -> usize {
        self.ring.mask + 1
    }

    /// Returns the value back when the queue is full.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let ring = &*self.ring;
        let tail = ring.tail.0.load(Ordering::Relaxed);
        let head = ring.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > ring.mask {
            return Err(value);
        }
        // SAFETY: slot `tail` is outside the consumer's [head, tail) range, so only we touch it.
        unsafe { (*ring.buf[tail & ring.mask].get()).write(value) };
        ring.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T: Copy + Send> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let ring = &*self.ring;
        let head = ring.head.0.load(Ordering::Relaxed);
        let tail = ring.tail.0.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        // SAFETY: the Acquire load of `tail` makes the producer's write to slot `head` visible, and
        // the producer will not reuse it until we publish head+1.
        let value = unsafe { (*ring.buf[head & ring.mask].get()).assume_init() };
        ring.head.0.store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub fn is_empty(&self) -> bool {
        let ring = &*self.ring;
        ring.head.0.load(Ordering::Relaxed) == ring.tail.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_queue_returns_the_value_and_keeps_fifo_order() {
        let (mut tx, mut rx) = channel::<u32>(4);
        assert_eq!(tx.capacity(), 4);
        for i in 0..4 {
            assert_eq!(tx.push(i), Ok(()));
        }
        assert_eq!(
            tx.push(99),
            Err(99),
            "overflow is reported, never silently dropped"
        );
        assert_eq!(rx.pop(), Some(0));
        assert_eq!(tx.push(4), Ok(()), "a freed slot is reusable (wraparound)");
        let drained: Vec<u32> = std::iter::from_fn(|| rx.pop()).collect();
        assert_eq!(drained, [1, 2, 3, 4]);
        assert!(rx.is_empty());
        assert_eq!(rx.pop(), None);
    }

    #[test]
    fn index_wraparound_keeps_working() {
        let (mut tx, mut rx) = channel::<usize>(2);
        for i in 0..10_000 {
            assert_eq!(tx.push(i), Ok(()));
            assert_eq!(rx.pop(), Some(i));
        }
    }

    #[test]
    fn cross_thread_delivery_is_ordered_and_lossless() {
        const N: u64 = 200_000;
        let (mut tx, mut rx) = channel::<u64>(64);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let producer_cancel = Arc::clone(&cancelled);
        let consumer_cancel = Arc::clone(&cancelled);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            for i in 0..N {
                let mut v = i;
                while let Err(back) = tx.push(v) {
                    if producer_cancel.load(Ordering::Acquire) {
                        return;
                    }
                    v = back;
                    std::thread::yield_now();
                }
            }
        });
        let consumer = std::thread::spawn(move || {
            let mut expected = 0;
            while expected < N && !consumer_cancel.load(Ordering::Acquire) {
                match rx.pop() {
                    Some(v) => {
                        assert_eq!(v, expected);
                        expected += 1;
                    }
                    None => std::thread::yield_now(),
                }
            }
            assert!(rx.is_empty());
            done_tx.send(expected).unwrap();
        });
        let completed = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        cancelled.store(true, Ordering::Release);
        producer.join().unwrap();
        consumer.join().unwrap();
        assert_eq!(completed.expect("queue delivery must finish"), N);
    }
}
