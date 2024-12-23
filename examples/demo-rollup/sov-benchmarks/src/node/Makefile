# Default values for num blocks and transactions per block
BLOCKS ?= 100
TXNS_PER_BLOCK ?= 1000

export BLOCKS
export TXNS_PER_BLOCK

criterion:
	@echo "Running criterion bench with $(TXNS_PER_BLOCK) transactions per block"
	@echo "Method: Criterion"
	@echo "Output: Criterion"
	@cd ../.. && cargo bench --bench=rollup_bench

basic:
	@echo "Running basic benchmark with $(BLOCKS) blocks and $(TXNS_PER_BLOCK) transactions per block"
	@echo "Method: Coarse Timers"
	@echo "Output: Standard"
	@cd ../.. && cargo bench --bench=rollup_coarse_measure

flamegraph:
	@echo "Running basic benchmark with $(BLOCKS) blocks and $(TXNS_PER_BLOCK) transactions per block"
	@echo "Method: Coarse Timers"
	@echo "Output: Flamegraph"
	@echo "WARNING: Flamegraph requires sudo. The Makefile does cleanup, but there is a unforeseen risk of files being owned by root after the script is done. The Makefile also does full cleanup so subsequent builds with default user will be from scratch."
	@read -p "Proceed (y/n): " REPLY; if [ $$REPLY = "y" ]; then \
		cd ../../ && sudo BLOCKS=$(BLOCKS) TXNS_PER_BLOCK=$(TXNS_PER_BLOCK) CARGO_PROFILE_BENCH_DEBUG=true cargo flamegraph -o benches/flamegraph.svg --bench rollup_coarse_measure --features="bench" && sudo rm -rf benches/demo_data ; \
	fi

flamegraph-criterion:
	@echo "Running basic benchmark with $(BLOCKS) blocks and $(TXNS_PER_BLOCK) transactions per block"
	@echo "Method: Criterion"
	@echo "Output: Flamegraph"
	@echo "WARNING: Flamegraph requires sudo. The Makefile does cleanup, but there is a unforeseen risk of files being owned by root after the script is done. The Makefile also does full cleanup so subsequent builds with default user will be from scratch."
	@read -p "Proceed (y/n): " REPLY; if [ $$REPLY = "y" ]; then \
		cd ../../ && sudo BLOCKS=$(BLOCKS) TXNS_PER_BLOCK=$(TXNS_PER_BLOCK) CARGO_PROFILE_BENCH_DEBUG=true cargo flamegraph -o benches/flamegraph.svg --bench=rollup_bench --features="bench"; \
	fi
