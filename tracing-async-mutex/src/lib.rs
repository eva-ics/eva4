use core::fmt;
use std::{
    ops::{Deref, DerefMut},
    sync::{atomic, Arc},
};

use parking_lot::Mutex as SyncMutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard, TryLockError};

#[derive(Serialize, Deserialize, Debug)]
pub struct MutexInfo<'a> {
    name: &'a str,
    owner: Option<Arc<String>>,
}

impl PartialEq for MutexInfo<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for MutexInfo<'_> {}

impl PartialOrd for MutexInfo<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MutexInfo<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(other.name)
    }
}

impl<'a> MutexInfo<'a> {
    pub fn for_tracing_mutex<T, D: From<usize> + fmt::Display>(
        name: &'a str,
        mutex: &TracingMutex<T>,
    ) -> Self {
        let owner = mutex.owner::<D>();
        MutexInfo {
            name,
            owner: owner.map(|o| o.to_string().into()),
        }
    }
    pub fn for_tracing_named_mutex<T>(name: &'a str, mutex: &TracingNamedMutex<T>) -> Self {
        let owner = mutex.owner();
        MutexInfo { name, owner }
    }
}

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

impl<T: ?Sized> Drop for TracingMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.owner.store(0, atomic::Ordering::Release);
    }
}

impl<T: ?Sized> Deref for TracingMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for TracingMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub struct TracingNamedMutex<T: ?Sized> {
    owner: Arc<SyncMutex<Option<Arc<String>>>>,
    inner: Mutex<T>,
}

impl<T: ?Sized> TracingNamedMutex<T> {
    pub fn blocking_lock<O: fmt::Display>(&self, owner: O) -> TracingNamedMutexGuard<'_, T> {
        let inner = self.inner.blocking_lock();
        *self.owner.lock() = Some(owner.to_string().into());
        TracingNamedMutexGuard {
            owner: self.owner.clone(),
            inner,
        }
    }
    pub fn const_new(t: T) -> Self
    where
        T: Sized,
    {
        Self {
            owner: <_>::default(),
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
    pub async fn lock<O: fmt::Display>(&self, owner: O) -> TracingNamedMutexGuard<'_, T> {
        let inner = self.inner.lock().await;
        *self.owner.lock() = Some(owner.to_string().into());
        TracingNamedMutexGuard {
            owner: self.owner.clone(),
            inner,
        }
    }
    pub fn new(t: T) -> Self
    where
        T: Sized,
    {
        Self {
            owner: <_>::default(),
            inner: Mutex::new(t),
        }
    }
    pub fn try_lock<O: fmt::Display>(
        &self,
        owner: O,
    ) -> Result<TracingNamedMutexGuard<'_, T>, TryLockError> {
        let inner = self.inner.try_lock()?;
        *self.owner.lock() = Some(owner.to_string().into());
        Ok(TracingNamedMutexGuard {
            owner: self.owner.clone(),
            inner,
        })
    }
    pub fn owner(&self) -> Option<Arc<String>> {
        self.owner.lock().clone()
    }
}

pub struct TracingNamedMutexGuard<'a, T: ?Sized> {
    owner: Arc<SyncMutex<Option<Arc<String>>>>,
    inner: MutexGuard<'a, T>,
}

impl<T: ?Sized> Drop for TracingNamedMutexGuard<'_, T> {
    fn drop(&mut self) {
        *self.owner.lock() = None;
    }
}

impl<T: ?Sized> Deref for TracingNamedMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for TracingNamedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
