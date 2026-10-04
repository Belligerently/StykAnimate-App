//! Locking that never blocks the interface thread.
//!
//! A browser's interface thread may not wait (`Atomics.wait` throws there), so it
//! cannot take a contended standard mutex. [`Shared::lock`] spins instead. That is
//! safe because every critical section in this crate is a few instructions: a
//! queue push, a map lookup, a pointer swap. Evaluation never runs under the lock.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

#[cfg(not(test))]
type LockGuard<'a, T> = MutexGuard<'a, T>;
#[cfg(test)]
pub(crate) struct LockGuard<'a, T> { guard: MutexGuard<'a, T>, traced: bool }
#[cfg(test)]
impl<T> std::ops::Deref for LockGuard<'_, T> { type Target = T; fn deref(&self) -> &T { &self.guard } }
#[cfg(test)]
impl<T> std::ops::DerefMut for LockGuard<'_, T> { fn deref_mut(&mut self) -> &mut T { &mut self.guard } }
#[cfg(test)]
impl<T> Drop for LockGuard<'_, T> { fn drop(&mut self) { crate::allocation::leave(self.traced); } }

pub struct Shared<T> {
    value: Mutex<T>,
    /// Counts publications of new work, so a worker never sleeps through one.
    sequence: AtomicU32,
    sleep: Mutex<()>,
    wake: Condvar,
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Shared<T> {
        Shared { value: Mutex::new(value), sequence: AtomicU32::new(0), sleep: Mutex::new(()), wake: Condvar::new() }
    }

    /// Takes the lock, spinning while another thread holds it.
    pub fn lock(&self) -> LockGuard<'_, T> {
        #[cfg(test)]
        crate::allocation::before_lock(self as *const Self as usize);
        loop {
            match self.value.try_lock() {
                Ok(guard) => {
                    #[cfg(not(test))] { return guard }
                    #[cfg(test)] { return LockGuard { guard, traced: crate::allocation::enter(self as *const Self as usize) } }
                },
                // A thread that panicked while holding the lock left consistent data:
                // critical sections here only move values.
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    let guard = poisoned.into_inner();
                    #[cfg(not(test))] { return guard }
                    #[cfg(test)] { return LockGuard { guard, traced: crate::allocation::enter(self as *const Self as usize) } }
                },
                Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
            }
        }
    }

    /// Wakes every sleeping worker. Call after publishing work, without the lock held.
    pub fn notify_all(&self) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        // Workers re-check the sequence under this mutex before sleeping. The interface
        // thread only tries it: if a worker is between its check and its sleep, the
        // timeout in `wait_for_work` bounds the delay.
        if let Ok(_guard) = self.sleep.try_lock() {
            self.wake.notify_all();
        } else {
            self.wake.notify_all();
        }
    }

    /// Sleeps the calling worker until work is published or `stopping` is set.
    pub fn wait_for_work(&self, stopping: &AtomicBool) {
        let seen = self.sequence.load(Ordering::SeqCst);
        let guard = match self.sleep.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if self.sequence.load(Ordering::SeqCst) != seen || stopping.load(Ordering::SeqCst) {
            return;
        }
        // The timeout covers a notification that raced the check above.
        let _ = self.wake.wait_timeout(guard, std::time::Duration::from_millis(50));
    }
}
