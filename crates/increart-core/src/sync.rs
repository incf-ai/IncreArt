//! Locks that outlive a panic. A thread that panics while holding a lock
//! poisons it; std then makes every later `lock()` an error. The data is
//! what the thread left (each lock here guards state that is whole between
//! statements), so the rest of the app goes on with it rather than failing
//! too.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

pub trait LockExt<T> {
    /// Locks, taking over a lock poisoned by a panic.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub trait WaitExt {
    /// [`Condvar::wait`], taking over a lock poisoned by a panic.
    fn wait_or_recover<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T>;
}

impl WaitExt for Condvar {
    fn wait_or_recover<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        self.wait(guard).unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_lock_poisoned_by_a_panic_is_taken_over() {
        let m = Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let mut g = m2.lock().unwrap();
            *g = 2;
            panic!("while holding it");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*m.lock_or_recover(), 2);
    }
}
