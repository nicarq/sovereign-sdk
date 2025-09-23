// sov_journal.h — guest shim for Ligetron with real SHA-256
// Contract: arg[1] = sha256(journal) (public), arg[2..] = private hints blob (chunked).
// Emits one line: "SOV_JOURNAL_HEX:<hex>"
// Note: Uses 1-based indexing to match Ligetron CLI.

#ifndef SOV_JOURNAL_H
#define SOV_JOURNAL_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#endif

// ======== RUNTIME HOOKS (provided by Ligetron WASM runtime) ========
// 1-based indices: i=1 is the first argument.
extern size_t ligetron_num_args(void);
extern size_t ligetron_arg_len(size_t i);
extern bool   ligetron_arg_copy(size_t i, uint8_t* out, size_t out_len);

// ======== TRAP/ABORT (portable) ========
#if defined(__wasm__) || defined(__wasm32__) || defined(__EMSCRIPTEN__)
#  define SOV_TRAP() __builtin_trap()
#else
#  define SOV_TRAP() do { fflush(stdout); fflush(stderr); exit(3); } while (0)
#endif

// ======== HEX UTILS ========

static inline char sov_hex_nibble(uint8_t x) {
    return "0123456789abcdef"[x & 0x0F];
}

// ======== REAL SHA-256 (portable, constant-time; no data-dependent branches) ========

static inline uint32_t sov_rotr32(uint32_t x, unsigned n) {
    return (x >> n) | (x << (32U - n));
}

#define SOV_CH(x,y,z)   (((x) & (y)) ^ (~(x) & (z)))
#define SOV_MAJ(x,y,z)  (((x) & (y)) ^ ((x) & (z)) ^ ((y) & (z)))
#define SOV_BSIG0(x)    (sov_rotr32((x),2) ^ sov_rotr32((x),13) ^ sov_rotr32((x),22))
#define SOV_BSIG1(x)    (sov_rotr32((x),6) ^ sov_rotr32((x),11) ^ sov_rotr32((x),25))
#define SOV_SSIG0(x)    (sov_rotr32((x),7) ^ sov_rotr32((x),18) ^ ((x) >> 3))
#define SOV_SSIG1(x)    (sov_rotr32((x),17) ^ sov_rotr32((x),19) ^ ((x) >> 10))

static inline uint32_t sov_read_be32(const uint8_t b[4]) {
    return ((uint32_t)b[0] << 24) | ((uint32_t)b[1] << 16) | ((uint32_t)b[2] << 8) | (uint32_t)b[3];
}

static inline void sov_write_be32(uint8_t out[4], uint32_t x) {
    out[0] = (uint8_t)(x >> 24);
    out[1] = (uint8_t)(x >> 16);
    out[2] = (uint8_t)(x >> 8);
    out[3] = (uint8_t)x;
}

static const uint32_t SOV_SHA256_IV[8] = {
    0x6a09e667U, 0xbb67ae85U, 0x3c6ef372U, 0xa54ff53aU,
    0x510e527fU, 0x9b05688cU, 0x1f83d9abU, 0x5be0cd19U
};

static const uint32_t SOV_SHA256_K[64] = {
    0x428a2f98U,0x71374491U,0xb5c0fbcfU,0xe9b5dba5U,0x3956c25bU,0x59f111f1U,0x923f82a4U,0xab1c5ed5U,
    0xd807aa98U,0x12835b01U,0x243185beU,0x550c7dc3U,0x72be5d74U,0x80deb1feU,0x9bdc06a7U,0xc19bf174U,
    0xe49b69c1U,0xefbe4786U,0x0fc19dc6U,0x240ca1ccU,0x2de92c6fU,0x4a7484aaU,0x5cb0a9dcU,0x76f988daU,
    0x983e5152U,0xa831c66dU,0xb00327c8U,0xbf597fc7U,0xc6e00bf3U,0xd5a79147U,0x06ca6351U,0x14292967U,
    0x27b70a85U,0x2e1b2138U,0x4d2c6dfcU,0x53380d13U,0x650a7354U,0x766a0abbU,0x81c2c92eU,0x92722c85U,
    0xa2bfe8a1U,0xa81a664bU,0xc24b8b70U,0xc76c51a3U,0xd192e819U,0xd6990624U,0xf40e3585U,0x106aa070U,
    0x19a4c116U,0x1e376c08U,0x2748774cU,0x34b0bcb5U,0x391c0cb3U,0x4ed8aa4aU,0x5b9cca4fU,0x682e6ff3U,
    0x748f82eeU,0x78a5636fU,0x84c87814U,0x8cc70208U,0x90befffaU,0xa4506cebU,0xbef9a3f7U,0xc67178f2U
};

static inline void sov_sha256_compress(uint32_t state[8], const uint8_t block[64]) {
    uint32_t w[64];
    for (unsigned t = 0; t < 16; ++t) {
        w[t] = sov_read_be32(block + 4U * t);
    }
    for (unsigned t = 16; t < 64; ++t) {
        w[t] = SOV_SSIG1(w[t - 2]) + w[t - 7] + SOV_SSIG0(w[t - 15]) + w[t - 16];
    }

    uint32_t a = state[0], b = state[1], c = state[2], d = state[3];
    uint32_t e = state[4], f = state[5], g = state[6], h = state[7];

    for (unsigned t = 0; t < 64; ++t) {
        uint32_t T1 = h + SOV_BSIG1(e) + SOV_CH(e, f, g) + SOV_SHA256_K[t] + w[t];
        uint32_t T2 = SOV_BSIG0(a) + SOV_MAJ(a, b, c);
        h = g;
        g = f;
        f = e;
        e = d + T1;
        d = c;
        c = b;
        b = a;
        a = T1 + T2;
    }

    state[0] += a; state[1] += b; state[2] += c; state[3] += d;
    state[4] += e; state[5] += f; state[6] += g; state[7] += h;
}

// One-shot SHA-256 API (compatible with Rust's sha2::Sha256 output)
static inline void sha256(const uint8_t* msg, size_t len, uint8_t out32[32]) {
    uint32_t state[8];
    for (int i = 0; i < 8; ++i) state[i] = SOV_SHA256_IV[i];

    const uint8_t* p = msg;
    size_t remaining = len;

    // Process full 64-byte blocks
    while (remaining >= 64) {
        sov_sha256_compress(state, p);
        p += 64;
        remaining -= 64;
    }

    // Final padding
    uint8_t block[128];
    size_t rem = remaining;
    if (rem) memcpy(block, p, rem);
    block[rem++] = 0x80;

    // If not enough room for 8-byte length, pad and compress, then start new block
    if (rem > 56) {
        memset(block + rem, 0, 64 - rem);
        sov_sha256_compress(state, block);
        rem = 0;
    }

    // Pad zeros until 56, then append 64-bit big-endian bit length
    memset(block + rem, 0, 56 - rem);
    uint64_t bitlen = (uint64_t)len * 8ULL;
    block[56] = (uint8_t)(bitlen >> 56);
    block[57] = (uint8_t)(bitlen >> 48);
    block[58] = (uint8_t)(bitlen >> 40);
    block[59] = (uint8_t)(bitlen >> 32);
    block[60] = (uint8_t)(bitlen >> 24);
    block[61] = (uint8_t)(bitlen >> 16);
    block[62] = (uint8_t)(bitlen >> 8);
    block[63] = (uint8_t)(bitlen);

    sov_sha256_compress(state, block);

    // Output big-endian digest
    for (int i = 0; i < 8; ++i) {
        sov_write_be32(out32 + 4 * i, state[i]);
    }
}

// ======== HINTS (reassemble arg[2..]) ========

// Load and reassemble private hints from args[2..N] into 'scratch'.
// Returns true on success; on success, *out_ptr points into 'scratch' and *out_len is total length.
static inline bool sov_load_hints(uint8_t** out_ptr, size_t* out_len,
                                  uint8_t* scratch, size_t scratch_len) {
    size_t n = ligetron_num_args();
    if (n <= 1) {
        *out_ptr = NULL;
        *out_len = 0;
        return true; // No hints is valid
    }
    
    size_t total = 0;
    for (size_t i = 2; i <= n; ++i) {
        total += ligetron_arg_len(i);
    }
    if (total > scratch_len) return false;
    
    size_t off = 0;
    for (size_t i = 2; i <= n; ++i) {
        size_t len = ligetron_arg_len(i);
        if (len == 0) continue;
        if (!ligetron_arg_copy(i, scratch + off, len)) return false;
        off += len;
    }
    
    *out_ptr = scratch; 
    *out_len = total;
    return true;
}

// ======== DIGEST (arg[1]) ========

static inline bool sov_load_digest(uint8_t out32[32]) {
    if (ligetron_num_args() < 1) return false;
    size_t len = ligetron_arg_len(1);
    if (len != 32) return false;
    return ligetron_arg_copy(1, out32, 32);
}

// ======== JOURNAL EMIT & CHECK ========

static inline void sov_emit_journal_hex(const uint8_t* data, size_t len) {
    // Print: SOV_JOURNAL_HEX:<hex>\n (lowercase, no 0x prefix)
    fputs("SOV_JOURNAL_HEX:", stdout);
    for (size_t i = 0; i < len; ++i) { 
        uint8_t b = data[i];
        putchar(sov_hex_nibble((uint8_t)(b >> 4)));
        putchar(sov_hex_nibble(b));
    }
    putchar('\n'); 
    fflush(stdout);
}

static inline void sov_assert_digest_matches(const uint8_t* journal, size_t len) {
    uint8_t expected[32], got[32];
    if (!sov_load_digest(expected)) { 
        // No expected digest provided: treat as the "first pass".
        // Do not enforce, just return so the host can read the journal
        // and re-run with the real digest.
        return;
    }
    
    sha256(journal, len, got);
    
    // Constant-time compare
    uint8_t diff = 0; 
    for (int i = 0; i < 32; ++i) {
        diff |= (uint8_t)(expected[i] ^ got[i]);
    }

    // If expected digest is all zeros, treat as first-pass sentinel -> don't abort.
    uint8_t zero = 0;
    for (int i = 0; i < 32; ++i) zero |= expected[i];
    if (diff != 0 && zero != 0) {
        fprintf(stderr, "Journal digest mismatch!\nExpected: ");
        for (int i = 0; i < 32; i++) fprintf(stderr, "%02x", expected[i]);
        fprintf(stderr, "\nGot:      ");
        for (int i = 0; i < 32; i++) fprintf(stderr, "%02x", got[i]);
        fprintf(stderr, "\n");
        fflush(stderr);
        // Non-zero failure/trap for the prover
        SOV_TRAP();
    }
}

// High-level API: emit journal and assert digest binding
static inline void sov_commit_and_check(const uint8_t* journal, size_t len) {
    sov_emit_journal_hex(journal, len);
    sov_assert_digest_matches(journal, len);
}

#ifdef __cplusplus
}
#endif

#endif // SOV_JOURNAL_H
