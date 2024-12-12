use indexmap::set::MutableValues;
use indexmap::{IndexMap, IndexSet};
use sov_modules_api::prelude::arbitrary::{self};

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

/// Utilities to easily generate random vectors of numbers
pub mod rng_utils {
    use rand::{RngCore, SeedableRng};

    /// Get a Vec of `num` bytes, seeded by `num` and  a salt value
    pub fn get_random_bytes(num: usize, salt: u128) -> Vec<u8> {
        let mut output = vec![0; num];
        randomize_buffer(&mut output, salt);
        output
    }

    /// Randomize the given buffer. The rng is seeded from the buffer's length and the salt
    pub fn randomize_buffer(buffer: &mut [u8], salt: u128) {
        // First, use seed_from_u64 to get a high quality rng. (Seeding yourself is hard because you need a high hamming weight!)
        let mut rng = rand_chacha::ChaChaRng::seed_from_u64(buffer.len() as u64);
        let mut seed = [0; 32];

        // Use the existing high quality rng to generate a high quality seed for the new one that we can modify
        rng.fill_bytes(&mut seed[..]);
        // Xor in the salt
        for (salt, seed) in salt.to_le_bytes().into_iter().zip(seed.iter_mut()) {
            *seed ^= salt;
        }
        // Use the final rng to overwrite the buffer
        rng = rand_chacha::ChaChaRng::from_seed(seed);
        rng.fill_bytes(buffer);
    }
}
