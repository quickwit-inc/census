//! ```rust
//! extern crate census;
//! use census::{Inventory, TrackedObject};
//!
//! fn main() {
//!
//!     let inventory = Inventory::new();
//!
//!     //  Each object tracked needs to be registered explicitely in the Inventory.
//!     //  A `TrackedObject<T>` wrapper is then returned.
//!     let one = inventory.track("one".to_string());
//!     let two = inventory.track("two".to_string());
//!
//!     // A snapshot  of the list of living instances can be obtained...
//!     // (no guarantee on their order)
//!     let living_instances: Vec<TrackedObject<String>> = inventory.list();
//!     assert_eq!(living_instances.len(), 2);
//!
//! }
//! ```


use std::sync::{Arc, RwLock};
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::borrow::Borrow;


/// The `Inventory` register and keeps track of all of the objects alive.
pub struct Inventory<T> {
    items: Arc<RwLock<Vec<TrackedObject<T>>>>,
}

impl<T> Clone for Inventory<T> {
    fn clone(&self) -> Self {
        Inventory {
            items: self.items.clone()
        }
    }
}

impl<T> Inventory<T> {

    /// Creates a new inventory object
    pub fn new() -> Inventory<T> {
        Inventory {
            items: Arc::default()
        }
    }

    /// Takes a snapshot of the list of tracked object.
    ///
    /// Note that the list is a simple `Vec` of tracked object.
    /// As a result, it is a consistent snapshot of the
    /// list of living instance at the time of the call,
    ///
    /// Obviously, instances may have been created after the call.
    /// They will obviously not appear in the snapshot.
    ///
    /// ```rust
    /// # extern crate census;
    /// # use census::{Inventory, TrackedObject};
    /// # fn main() {
    /// #
    /// let inventory = Inventory::new();
    ///
    /// let one = inventory.track("one".to_string());
    /// let living_instances: Vec<TrackedObject<String>> = inventory.list();
    /// let two = inventory.track("two".to_string());
    ///
    /// // our snapshot is a bit old.
    /// assert_eq!(living_instances.len(), 1);
    ///
    /// // a fresher snapshot would contain our new element.
    /// assert_eq!(inventory.list().len(), 2);
    /// # }
    /// ```
    ///
    /// Also, the instance in the snapshot itself
    /// are considered "living".
    ///
    /// As a result, as long as a snapshot is not dropped,
    /// all of its instances will be part of the inventory.
    ///
    /// ```rust
    /// # extern crate census;
    /// # use census::{Inventory, TrackedObject};
    /// # fn main() {
    /// #
    /// let inventory = Inventory::new();
    ///
    /// let one = inventory.track("one".to_string());
    /// let living_instances: Vec<TrackedObject<String>> = inventory.list();
    ///
    /// // let's drop one here
    /// drop(one);
    ///
    /// // The instance is technically still in the inventory
    /// // as our previous snapshot is extending its life...
    /// assert_eq!(inventory.list().len(), 1);
    ///
    /// // If we drop our previous snapshot however...
    /// drop(living_instances);
    ///
    /// // `one` is really untracked.
    /// assert!(inventory.list().is_empty());
    /// # }
    /// ```
    ///
    pub fn list(&self) -> Vec<TrackedObject<T>> {
        self.items.read()
            .expect("Lock poisoned")
            .clone()
    }

    /// Starts tracking a given `T` object.
    pub fn track(&self, t: T) -> TrackedObject<T> {
        let self_clone: Inventory<T> = (*self).clone();
        let mut wlock = self.items
            .write()
            .expect("Inventory lock poisoned on write");
        let idx = wlock.len();
        let managed_object = TrackedObject {
            census: self_clone,
            inner: Arc::new(Inner {
                val:t,
                count: AtomicUsize::new(0),
                idx: AtomicUsize::new(idx),
            })
        };
        wlock.push(managed_object.clone());
        managed_object
    }

    fn remove(&self, el: &TrackedObject<T>) {
        let mut wlock = self.items
            .write()
            .expect("Inventory lock poisoned on read");
        // We need to double check that the ref count is 0, as
        // the obj could have been cloned in right before taking the lock,
        // by calling a `list` for instance.
        let ref_count = el.inner.count.load(Ordering::SeqCst);
        if ref_count != 0 {
            return;
        }

        let pos = el.index();

        // just pop if this was the last element
        if pos + 1 == wlock.len() {
            wlock.pop();
        } else {
            wlock.swap_remove(pos);
            wlock[pos].set_index(pos);
        }
    }
}

impl<T> Drop for TrackedObject<T> {
    fn drop(&mut self) {
        let count_before = self.inner.count.fetch_sub(1, Ordering::SeqCst);
        if count_before == 1 {
            // this was the last reference.
            // Let's remove our object from the census.
            self.census.remove(self);
        }
    }
}

impl<T> Clone for TrackedObject<T> {
    fn clone(&self) -> Self {
        self.inner.count.fetch_add(1, Ordering::SeqCst);
        TrackedObject {
            census: self.census.clone(),
            inner: self.inner.clone(),
        }
    }
}

struct Inner<T> {
    val: T,
    count: AtomicUsize,
    idx: AtomicUsize,
}

/// Your tracked object.
///
///
/// A tracked object contains reference counting logic and an
/// `Arc` to your object.
///
/// It is cloneable but calling clone will not clone
/// your internal object.
///
/// Your object cannot be mutated. You can borrow it using
/// the `Deref` interface.
pub struct TrackedObject<T> {
    census: Inventory<T>,
    inner: Arc<Inner<T>>,
}

impl<T> TrackedObject<T> {

    fn index(&self) -> usize {
        self.inner.idx.load(Ordering::SeqCst)
    }

    fn set_index(&self, pos: usize) {
        self.inner.idx.store(pos, Ordering::SeqCst);
    }


    /// Creates a new object from an existing one.
    ///
    /// The new object will be registered
    /// in your original object's inventory.
    ///
    /// ```rust
    /// # extern crate census;
    /// # use census::{Inventory, TrackedObject};
    /// # fn main() {
    /// #
    /// let inventory = Inventory::new();
    ///
    /// let seven = inventory.track(7);
    /// let fourteen = seven.map(|i| i * 2);
    /// assert_eq!(*fourteen, 14);
    ///
    /// let living_instances = inventory.list();
    /// assert_eq!(living_instances.len(), 2);
    /// # }
    /// ```
    pub fn map<F>(&self, f: F) -> TrackedObject<T>
        where F: FnOnce(&T)->T {
        let t = f(&*self);
        self.census.track(t)
    }
}

impl<T> Deref for TrackedObject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.val
    }
}

impl<T> AsRef<T> for TrackedObject<T> {
    fn as_ref(&self) -> &T {
        &self.inner.val
    }
}

impl<T> Borrow<T> for TrackedObject<T> {
    fn borrow(&self) -> &T {
        &self.inner.val
    }
}

#[cfg(test)]
mod tests {

    use super::Inventory;
    use std::thread;

    #[test]
    fn test_census_map() {
        let census = Inventory::new();
        let a = census.track(1);
        let _b = a.map(|v| v*7);
        assert_eq!(
            census
                .list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1, 7]
        );
    }


    #[test]
    fn test_census() {
        let census = Inventory::new();
        let _a = census.track(1);
        let _b = census.track(3);
        assert_eq!(
            census
                .list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1, 3]);
    }

    #[test]
    fn test_census_2() {
        let census = Inventory::new();
        {
            let _a = census.track(1);
            let _b = census.track(3);
            // dropping both here
        }
        assert!(census.list().is_empty());
    }

    #[test]
    fn test_census_3() {
        let census = Inventory::new();
        let a = census.track(1);
        let _a2 = a.clone();
        drop(a);
        assert_eq!(
            census.list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn test_census_list_extends_life() {
        let census = Inventory::new();
        let a = census.track(1);
        let living = census.list();
        assert_eq!(living.len(), 1);
        drop(a);
        let living_2 = census.list();
        assert_eq!(living_2.len(), 1);
        drop(living_2);
        drop(living);
        assert!(census.list().is_empty());
    }

    #[test]
    fn test_census_race_condition() {
        let census = Inventory::new();
        let census_clone = census.clone();
        thread::spawn(move || {
            for _ in 0..1_000 {
                let _a = census_clone.track(1);
            }
        });
        for _ in 0..10_000 {
            census.list();
        }
    }
}