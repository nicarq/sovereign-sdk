# Contains INFLUX_ORG_ID and INFLUX_AUTH_TOKEN.
-include config.env
export

# Can be "small", "standard" or "large"
SIZE ?= small
# Should be greater than 0. Usually below 30.
NUM_THREADS_LOGS ?= 20

NUM_THREADS ?= 10

NUM_SLOTS ?= 100

DOCKER_PATH ?= ../../../docker

LOCALHOST ?= 127.0.0.1

start_metrics_container:
	@echo "Starting metrics container. Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@sudo docker compose --env-file config.env -f $(DOCKER_PATH)/docker-compose.obs.yml up -d --remove-orphans

stop_metrics_container:
	@echo "Stopping metrics container..."
	@sudo docker compose --env-file config.env -f $(DOCKER_PATH)/docker-compose.obs.yml down

generate_benches:
	@echo "Generating $(SIZE) benchmarks with $(NUM_SLOTS) slots..."
	@echo "Benchmark files stored at path: ./src/bench_files/generated/$(SIZE)"
	@mkdir -p ./src/bench_files/generated/$(SIZE)
	@cargo build -r --bin bench_generator 
	@./target/release/bench_generator -s $(NUM_SLOTS) -p ./src/bench_files/generated/$(SIZE) $(SIZE)

bench_runner:
	@echo "Running benchmark files path: ./src/bench_files/generated/$(SIZE)"
	@mkdir -p ./src/metrics/generated/$(SIZE)
	@cargo build -r --bench bench_runner 

run_benches: bench_runner
	@output=$$(ls -t ./target/release/deps/bench_runner-* | head -n 1) && \
	$$output -t $(NUM_THREADS) -p ./src/bench_files/generated/$(SIZE)

run_benches_with_logs: bench_runner
	@echo "Logging threads: $(NUM_THREADS_LOGS)"
	@output=$$(ls -t ./target/release/deps/bench_runner-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -t $(NUM_THREADS) -l $(NUM_THREADS_LOGS)

run_benches_with_encoded_metrics: start_metrics_container bench_runner
	@echo "Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@echo "Number of benchmark threads $(NUM_THREADS)"
	@echo "Metrics will be stored at path: ./src/metrics/generated/$(SIZE)"
	@output=$$(ls -t ./target/release/deps/bench_runner-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -t $(NUM_THREADS) \
		metrics -t $(LOCALHOST):$(TELEGRAF_PORT) -i $(LOCALHOST):$(INFLUX_PORT) \
		-o ./src/metrics/generated/$(SIZE) \
		--influx-auth-token $(INFLUX_AUTH_TOKEN) --influx-org-id $(INFLUX_ORG_ID) -e

run_benches_with_zk_metrics: start_metrics_container bench_runner
	@echo "Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@echo "Number of benchmark threads $(NUM_THREADS)"
	@echo "Metrics will be stored at path: ./src/metrics/generated/$(SIZE)"
	@output=$$(ls -t ./target/release/deps/bench_runner-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -t $(NUM_THREADS) \
		metrics -t $(LOCALHOST):$(TELEGRAF_PORT) -i $(LOCALHOST):$(INFLUX_PORT) \
		-o ./src/metrics/generated/$(SIZE) \
		--influx-auth-token $(INFLUX_AUTH_TOKEN) --influx-org-id $(INFLUX_ORG_ID) \
		measurements zk_vm zk_proving

run_benches_with_gas_constants_and_zk_metrics: start_metrics_container 
	@echo "Running benchmark files path: ./src/bench_files/generated/$(SIZE)."
	@mkdir -p ./src/metrics/generated/$(SIZE)
	@cargo build -r --bench bench_runner --features gas-constant-estimation
	@echo "Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@echo "Number of benchmark threads $(NUM_THREADS)"
	@echo "Metrics will be stored at path: ./src/metrics/generated/$(SIZE)"
	@output=$$(ls -t ./target/release/deps/bench_runner-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -t $(NUM_THREADS) \
		metrics -t $(LOCALHOST):$(TELEGRAF_PORT) -i $(LOCALHOST):$(INFLUX_PORT) \
		-o ./src/metrics/generated/$(SIZE) \
		--influx-auth-token $(INFLUX_AUTH_TOKEN) --influx-org-id $(INFLUX_ORG_ID) \
		measurements zk_vm zk_proving gas_constant_usage