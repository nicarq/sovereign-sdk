.PHONY: help

EXTRA_DIRS := crates/fuzz examples/demo-rollup/provers/risc0/guest-mock examples/demo-rollup/provers/risc0/guest-celestia

# Default is 256[^1], but `proptest` can be slow[^2] and local testing is not
# the place to run expensive, long-running tests with property checking. That's
# better left to CI.
#
# Ideally, one day we'd be able to increase this number without affecting
# developer productivity.
#
# [^1]: https://proptest-rs.github.io/proptest/proptest/tutorial/config.html
# [^2]: https://github.com/proptest-rs/proptest/issues/286
export PROPTEST_CASES := 50

help: ## Display this help message
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## Build the project
	@cargo build

clean: ## Cleans compiled
	@cargo clean

total-clean: clean
total-clean:
	$(MAKE) -C examples/demo-rollup clean;
	$(MAKE) -C examples/demo-rollup clean-wallet;
	@for dir in $(EXTRA_DIRS); do \
    	echo "Running cargo clean in $$dir"; \
    	(cd $$dir && cargo clean); \
    done;
	rm -rf "examples/demo-rollup/tests/evm/uniswap/node_modules"


test-legacy: ## Runs test suite with output from tests printed
	@cargo test -- --nocapture -Zunstable-options --report-time

test:  ## Runs test suite using next test
	@cargo nextest run --workspace --all-features --status-level skip

test-default-features:  ## Runs test suite using default features
	@cargo nextest run --workspace --status-level skip

install-dev-tools:  ## Installs all necessary cargo helpers
install-dev-tools: install-risc0-toolchain
	## Backup VS Code settings to `.vscode/settings.json.bak`.
	cp .vscode/settings.json .vscode/settings.json.bak || true
	## Install the default suggested VS Code settings.
	cp .vscode/settings.default.json .vscode/settings.json
	cargo install cargo-llvm-cov
	cargo install cargo-hack
	cargo install cargo-udeps
	cargo install cargo-deny
	cargo install flaky-finder
	cargo install cargo-nextest --locked
	cargo install cargo-risczero
	cargo install zepter
	rustup target add wasm32-unknown-unknown

install-risc0-toolchain:
	cargo risczero install --version r0.1.79.0
	@echo "Risc0 toolchain version:"
	cargo +risc0 --version

install-sp1-toolchain:
	curl -L https://sp1.succinct.xyz | bash
	~/.sp1/bin/sp1up
	~/.sp1/bin/cargo-prove prove --version
	~/.sp1/bin/cargo-prove prove install-toolchain
	@echo "SP1 toolchain version:"
	cargo +succinct --version

lint:  ## cargo check and clippy. Skip clippy on guest code since it's not supported by risc0
	## fmt first, because it's the cheapest
	cargo +nightly fmt --all --check
	cargo check --all-targets --all-features
	## Invokes Zepter multiple times because fixes sometimes unveal more underlying issues.
	zepter
	zepter
	zepter
	$(MAKE) check-fuzz
	SKIP_GUEST_BUILD=1 cargo clippy --all-targets --all-features

cargo-deny-check-licenses:
	cargo deny check licenses

cargo-deny-check:   ## Runs a global cargo-deny check, not just the licenses.
	cargo deny check --hide-inclusion-graph

extra-check:   ## cargo check in non attached crates
	cargo check
	@for dir in $(EXTRA_DIRS); do \
		echo "Running cargo check in $$dir"; \
		(cd $$dir && cargo check); \
	done

lint-fix:  ## cargo fmt, fix and clippy. Skip clippy on guest code since it's not supported by risc0
	cargo +nightly fmt --all
	cargo fix --allow-dirty
	SKIP_GUEST_BUILD=1 cargo clippy --fix --allow-dirty

check-features: ## Checks that project compiles with all combinations of features.
	cargo hack check --workspace --feature-powerset --exclude-features default --all-targets

check-features-default-targets:
	cargo hack check --workspace --feature-powerset --exclude-features default

check-fuzz: ## Checks that fuzz member compiles
	$(MAKE) -C crates/fuzz check

find-unused-deps: ## Prints unused dependencies for project. Note: requires nightly
	cargo +nightly udeps --all-targets --all-features

find-flaky-tests:  ## Runs tests over and over to find if there's flaky tests
	flaky-finder -j16 -r320 --continue "cargo test -- --nocapture"

coverage: ## Coverage in lcov format
	SP1_PROVER=mock cargo llvm-cov nextest --locked --all-features --lcov --output-path lcov.info

coverage-html: ## Coverage in HTML format
	SP1_PROVER=mock cargo nextest llvm-cov --locked --all-features --html

dry-run-publish: 
	yq '.[]' packages_to_publish.yml | xargs -I _ cargo publish --allow-dirty --dry-run -p _

docs:  ## Generates documentation locally
	cargo doc --open
