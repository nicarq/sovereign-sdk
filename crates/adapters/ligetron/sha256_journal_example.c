// sha256_journal_example.c
// A minimal journal-aware C program that demonstrates proper journal usage
// Computes sha256("hello world") and commits that 32-byte digest as the journal

#include <string.h>
#include "include/sov_journal.h"

// Compute sha256("hello world") on-device and commit that 32-byte digest.
int main(void) {
    static const uint8_t msg[] = "hello world";
    uint8_t digest[32];
    
    // Compute SHA256 of the message using our real implementation
    sov_sha256(msg, sizeof(msg) - 1, digest);

    // Publish the digest and check it matches arg[1].
    // This will:
    // 1. Emit "SOV_JOURNAL_HEX:<hex>" to stdout
    // 2. Verify that arg[1] == sha256(digest)
    // 3. Exit with code 3 if mismatch, or return 0 if success
    sov_commit_and_check_or_abort(digest, sizeof(digest));
    
    return 0;
}
