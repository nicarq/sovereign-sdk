use indexmap::set::MutableValues;
use indexmap::{IndexMap, IndexSet};
use sov_modules_api::prelude::arbitrary::{self};

/// Generates a value at random from the provided input. This value will be uniformly random
/// if the input byte stream is random.
pub trait RandomUniform: arbitrary::unstructured::Int {
    /// Pick a value in the provided range uniformly at random. Panics if the range is empty.
    fn in_range(
        range: std::ops::RangeInclusive<Self>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self> {
        u.int_in_range(range)
    }
}

/// A helper trait for borrowing an entry at random from a collection
pub trait PickRandom {
    /// The type yielded from the collection. Typically an &'a T, where T is the type
    /// stored in the collection.
    type Item<'a>
    where
        Self: 'a;
    /// Pick an item at random from a collection
    ///
    /// # Panics
    ///
    /// Panics if the collection is empty.
    fn random_entry(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>>;
}

/// A helper trait for mutably borrowing an entry at random from a collection
pub trait PickRandomMut {
    /// The type yielded from the collection. Typically an &'a mut T, where T is the type
    /// stored in the collection.
    type Item<'a>
    where
        Self: 'a;
    /// Pick an item at random from a collection
    ///
    /// # Panics
    ///
    /// Panics if the collection is empty.
    fn random_entry_mut(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>>;
}

impl<K, V> PickRandom for IndexMap<K, V>
where
    K: 'static,
    V: 'static,
{
    type Item<'a> = (&'a K, &'a V);

    fn random_entry(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get_index(idx).expect("Index is in range"))
    }
}

impl<T> PickRandom for IndexSet<T>
where
    T: 'static,
{
    type Item<'a> = &'a T;

    fn random_entry(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get_index(idx).expect("Index is in range"))
    }
}

impl<T> PickRandom for Vec<T>
where
    T: 'static,
{
    type Item<'a> = &'a T;

    fn random_entry(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get(idx).expect("Index is in range"))
    }
}

impl<K, V> PickRandomMut for IndexMap<K, V>
where
    K: 'static,
    V: 'static,
{
    type Item<'a> = (&'a K, &'a mut V);

    fn random_entry_mut(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get_index_mut(idx).expect("Index is in range"))
    }
}

impl<T> PickRandomMut for IndexSet<T>
where
    T: 'static,
{
    type Item<'a> = &'a mut T;

    fn random_entry_mut(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get_index_mut2(idx).expect("Index is in range"))
    }
}

impl<T> PickRandomMut for Vec<T>
where
    T: 'static,
{
    type Item<'a> = &'a mut T;

    fn random_entry_mut(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self::Item<'_>> {
        let idx = u.choose_index(self.len())?;
        Ok(self.get_mut(idx).expect("Index is in range"))
    }
}
