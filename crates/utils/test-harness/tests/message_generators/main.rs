mod bank;

mod basic;

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
