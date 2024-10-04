use std::ops::{Add, Shr, Sub};

use sov_modules_api::prelude::arbitrary::{self, Arbitrary};

/// Generates a value at random from the provided input. This value will be uniformly random
/// if the input byte stream is random.
pub trait RandomUniform: Clone + Sized + Sub<Output = Self> + Add<Output = Self> {
    /// Select a value in range 0..max uniformly at random.
    fn less_than(max: &Self, u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self>;

    /// Pick a value in the provided range uniformly at random
    fn in_range(
        range: std::ops::Range<Self>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Self> {
        let len = range.end - range.start.clone();
        Ok(Self::less_than(&len, u)? + range.start)
    }
}

macro_rules! impl_random_uniform_for_uint {
    ($t:ty) => {
        impl RandomUniform for $t {
            fn less_than(
                max: &Self,
                u: &mut arbitrary::Unstructured<'_>,
            ) -> arbitrary::Result<Self> {
                // Say our max value is 10 (0b1010). This has 60 leading zeros. To compute the appropriate
                // bitmask, we take 0b1111 1111 1111 1111 ...  and shift right by that number of leading zeros.
                // That yields 0b0000 ... 0000 1111.
                // Now we can generate a value uniformly at random and simply mask off the uppermost bits.
                // This yields a value in range 0..max.next_po2(), which is at most twice as large as our target value.
                // If this value is in our target range, return it. Otherwise, retry. The probability of failure on any
                // loop iteration is at most 1/2, so this will converge quickly.
                let max = max - 1;
                let mask = Self::MAX.shr(max.leading_zeros());
                loop {
                    let value = Self::arbitrary(u)? & mask;
                    if value <= max {
                        return Ok(value);
                    }
                }
            }
        }
    };
}

impl_random_uniform_for_uint!(u8);
impl_random_uniform_for_uint!(u16);
impl_random_uniform_for_uint!(u32);
impl_random_uniform_for_uint!(usize);
impl_random_uniform_for_uint!(u64);
impl_random_uniform_for_uint!(u128);

#[cfg(test)]
mod tests {
    use sov_modules_api::prelude::arbitrary;

    use super::RandomUniform;

    #[test]
    fn test_random_in_range() {
        let mut input = [0u8; 256];
        input
            .iter_mut()
            .enumerate()
            .for_each(|(idx, item)| *item = idx as u8);

        let mut u = arbitrary::Unstructured::new(&input);
        for _ in 0..128 {
            match u8::in_range(5..10, &mut u) {
                Ok(item) => {
                    assert!(item >= 5);
                    assert!(item < 10);
                }
                Err(e) => panic!("{}", e),
            }
        }
    }
}
