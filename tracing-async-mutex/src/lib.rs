use std::{
    ops::{Deref, DerefMut},
    sync::{atomic, Arc},
};

use tokio::sync::{Mutex, MutexGuard, TryLockError};

pub struct TracingMutex<T: ?Sized> {
    owner: Arc<atomic::AtomicUsize>,
    inner: Mutex<T>,
}

#[inline]
fn check_owner(u: usize) {
    assert!(u != 0, "Owner can not be 0");
}

impl<T: ?Sized> TracingMutex<T> {
    pub fn blocking_lock<O: AsRef<usize>>(&self, owner: O) -> TracingMutexGuard<'_, T> {
        let owner = *owner.as_ref();
        check_owner(owner);
        let inner = self.inner.blocking_lock();
        self.owner.store(owner, atomic::Ordering::Release);
        TracingMutexGuard {
            owner: self.owner.clone(),
            inner,
        }
    }
    pub fn const_new(t: T) -> Self
    where
        T: Sized,
    {
        Self {
            owner: Arc::new(atomic::AtomicUsize::new(0)),
            inner: Mutex::const_new(t),
        }
    }
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }
    pub fn into_inner(self) -> T
    where
        T: Sized,
    {
        self.inner.into_inner()
    }
    pub async fn lock<O: AsRef<usize>>(&self, owner: O) -> TracingMutexGuard<'_, T> {
        let owner = *owner.as_ref();
        check_owner(owner);
        let inner = self.inner.lock().await;
        self.owner.store(owner, atomic::Ordering::Release);
        TracingMutexGuard {
            owner: self.owner.clone(),
            inner,
        }
    }
    pub fn new(t: T) -> Self
    where
        T: Sized,
    {
        Self {
            owner: Arc::new(atomic::AtomicUsize::new(0)),
            inner: Mutex::new(t),
        }
    }
    pub fn try_lock<O: AsRef<usize>>(
        &self,
        owner: O,
    ) -> Result<TracingMutexGuard<'_, T>, TryLockError> {
        let owner = *owner.as_ref();
        check_owner(owner);
        let inner = self.inner.try_lock()?;
        self.owner.store(owner, atomic::Ordering::Release);
        Ok(TracingMutexGuard {
            owner: self.owner.clone(),
            inner,
        })
    }
    pub fn owner<O: From<usize>>(&self) -> Option<O> {
        let owner = self.owner.load(atomic::Ordering::Acquire);
        (owner != 0).then(|| owner.into())
    }
}

pub struct TracingMutexGuard<'a, T: ?Sized> {
    owner: Arc<atomic::AtomicUsize>,
    inner: MutexGuard<'a, T>,
}

impl<'a, T: ?Sized> Drop for TracingMutexGuard<'a, T> {
    fn drop(&mut self) {
        self.owner.store(0, atomic::Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for TracingMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a, T: ?Sized> DerefMut for TracingMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
