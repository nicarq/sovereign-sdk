## Test Harness Library

This library contains the functionality required by the `test-harness` binary. Split this way, bench marking suites may use this library as a `dev-dependency` in order to use its message generators, or module authores to facilitate implementing the `MessageSender` trait so that their module's `CallMessage`s may be included in this testing framework.
