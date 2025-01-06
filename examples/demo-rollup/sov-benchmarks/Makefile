# Can be "small", "standard" or "large"
SIZE ?= small

generate_and_run_benches:
	@echo "Generating and running $(SIZE) benchmarks..."
	@mkdir -p ./src/bench_files/generated/$(SIZE)
	@cargo build -r --bin bench_generator 
	@./target/release/bench_generator -p ./src/bench_files/generated/$(SIZE) $(SIZE)
	@cargo build -r --bench build_metrics
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE)