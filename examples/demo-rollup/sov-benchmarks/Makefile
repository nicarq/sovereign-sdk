# Contains INFLUX_ORG_ID and INFLUX_AUTH_TOKEN.
-include SECRETS.env
export

# Can be "small", "standard" or "large"
SIZE ?= small
# Should be greater than 0. Usually below 30.
NUM_THREADS_LOGS ?= 20

DOCKER_PATH ?= ../../../docker

LOCALHOST ?= 127.0.0.1

TELEGRAF_PORT ?= 8094
INFLUX_PORT ?= 8086

start_metrics_container:
	@echo "Starting metrics container. Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@TELEGRAF_PORT=$(TELEGRAF_PORT) INFLUX_PORT=$(INFLUX_PORT) sudo docker compose -f $(DOCKER_PATH)/docker-compose.obs.yml up -d

stop_metrics_container:
	@echo "Stopping metrics container..."
	@sudo docker compose -f $(DOCKER_PATH)/docker-compose.obs.yml down

generate_benches:
	@echo "Generating $(SIZE) benchmarks..."
	@echo "Benchmark files stored at path: ./src/bench_files/generated/$(SIZE)"
	@mkdir -p ./src/bench_files/generated/$(SIZE)
	@cargo build -r --bin bench_generator 
	@./target/release/bench_generator -p ./src/bench_files/generated/$(SIZE) $(SIZE)

build_metrics:
	@echo "Running benchmark files path: ./src/bench_files/generated/$(SIZE)"
	@cargo build -r --bench build_metrics

run_benches: build_metrics
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE)

run_benches_with_logs: build_metrics
	@echo "Logging threads: $(NUM_THREADS_LOGS)"
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) -l $(NUM_THREADS_LOGS)

run_benches_with_encoded_metrics: start_metrics_container build_metrics
	@echo "Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@echo "Metrics will be stored at path: ./src/metrics/generated/$(SIZE)"
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) \
		metrics -t $(LOCALHOST):$(TELEGRAF_PORT) -i $(LOCALHOST):$(INFLUX_PORT) \
		-o ./src/metrics/generated/$(SIZE) \
		--influx-auth-token $(INFLUX_AUTH_TOKEN) --influx-org-id $(INFLUX_ORG_ID) -e

run_benches_with_zk_metrics: start_metrics_container build_metrics
	@echo "Telegraf port: $(TELEGRAF_PORT), Influx port: $(INFLUX_PORT)..."
	@echo "Metrics will be stored at path: ./src/metrics/generated/$(SIZE)"
	@output=$$(ls -t ./target/release/deps/build_metrics-* | head -n 1) && \
	$$output -p ./src/bench_files/generated/$(SIZE) \
		metrics -t $(LOCALHOST):$(TELEGRAF_PORT) -i $(LOCALHOST):$(INFLUX_PORT) \
		-o ./src/metrics/generated/$(SIZE) \
		--influx-auth-token $(INFLUX_AUTH_TOKEN) --influx-org-id $(INFLUX_ORG_ID) \
		measurements zk_vm zk_proving
