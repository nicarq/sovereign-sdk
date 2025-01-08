# Can be "small", "standard" or "large"
SIZE ?= small
# Should be greater than 0. Usually below 30.
NUM_THREADS_LOGS ?= 20

generate_benches:
	@echo "Generating $(SIZE) benchmarks..."
	@mkdir -p ./src/bench_files/generated/$(SIZE)
	@cargo build -r --bin bench_generator 
	@./target/release/bench_generator -p ./src/bench_files/generated/$(SIZE) $(SIZE)

run_benches:
	@echo "Running benchmarks without log assertions..."
	@cargo build -r --bench build_metrics
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE)

generate_and_run_benches: generate_benches run_benches

run_benches_with_logs:
	@echo "Running benchmarks with logs using $(NUM_THREADS_LOGS) threads..."
	@cargo build -r --bench build_metrics
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -l $(NUM_THREADS_LOGS)

generate_and_run_benches_with_logs: generate_benches run_benches_with_logs