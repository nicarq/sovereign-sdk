.PHONY: clean start stop print-jwt-token logs

PROJECT_ROOT := $(shell git rev-parse --show-toplevel)
DOCKER_COMPOSE_DIR := $(PROJECT_ROOT)/docker
CREDENTIALS_DIR := $(DOCKER_COMPOSE_DIR)/credentials
DOCKER_COMPOSE := docker compose -f $(DOCKER_COMPOSE_DIR)/docker-compose.yml

TEST_PRIVATE_KEY_PATH := $(PROJECT_ROOT)/examples/test-data/keys/minter_private_key.json
SOV_CLI_REL_PATH := $(PROJECT_ROOT)/target/debug/sov-cli

P2P_NETWORK := private
KEY_NAME := bridge-0

JWT_TOKEN_PATH := $(CREDENTIALS_DIR)/$(KEY_NAME).jwt
get_token := $(DOCKER_COMPOSE) exec -it sequencer-0 celestia bridge auth admin --p2p.network "private"
timestamp := $(shell date "+%Y-%m-%d %H:%M:%S")


clean: check-docker
clean:
	@echo "Stopping local celestia network setup"
	$(DOCKER_COMPOSE) down --volumes --rmi local
	rm -f "$(CREDENTIALS_DIR)/$(KEY_NAME).jwt"
	rm -rf "./demo_data"
	rm -f demo_rollup_config.toml
	rm -rf "~/.sov-cli-wallet"
	$(eval path := ./$(shell awk -F'=' '/^path/ {print $$2}' celestia_rollup_config.toml | tr -d '[:space:]"\n'))
	@if [ -z "${path}" ] || [ "${path}" = "./" ]; then \
		echo "Path is empty or too short, not safe to remove"; \
		exit 1; \
	fi
	@echo removing celestia rollup database "${path}"
	rm -rf "${path}"
	$(eval path := ./$(shell awk -F'=' '/^path/ {print $$2}' mock_rollup_config.toml | tr -d '[:space:]"\n'))
	@if [ -z "${path}" ] || [ "${path}" = "./" ]; then \
		echo "Path is empty or too short, not safe to remove"; \
		exit 1; \
	fi
	@echo removing mock rollup database "${path}"
	rm -rf "${path}"

start: check-docker
start:
	$(DOCKER_COMPOSE) up --build --force-recreate -d
	@echo "$(timestamp): waiting for sequencer-0 to start..."
	@while true; do \
		health=$$(docker inspect --format='{{.State.Health.Status}}' $$($(DOCKER_COMPOSE) ps -q sequencer-0)); \
		if [ "$$health" = "healthy" ]; then \
			echo "$(timestamp): sequencer-0 is healthy"; \
			break; \
		else \
			echo "$(timestamp): health == '$$health': Waiting for sequencer-0 to up and running..."; \
		fi; \
		sleep 3; \
	done
	@echo "Services are ready, preparing configuration"
	@while [ -z "$$TOKEN" ]; do \
		echo "$(timestamp): Waiting JWT for token..."; \
		TOKEN=$$($(get_token) 2>/dev/null); \
		sleep 1; \
	done; \
	echo "$(timestamp): token received: $$TOKEN";
	@TOKEN=$$($(get_token) 2>/dev/null); \
   	cat celestia_rollup_config.toml | \
   	sed 's/^\(celestia_rpc_auth_token = \)"[^"]*"/\1"'$$TOKEN'"/' > demo_rollup_config.toml;

stop: check-docker
stop:
	$(DOCKER_COMPOSE) down

logs:
	$(DOCKER_COMPOSE) logs -f

print-jwt-token:
	@echo "Token: $(get_token)"

check-docker:
	@command -v docker > /dev/null 2>&1 || { echo "Docker is not installed"; exit 1; }

check-sov-cli:
	@command -v $(SOV_CLI_REL_PATH) > /dev/null 2>&1 || { echo "sov-cli is not build, run `make build`"; exit 1; }

celestia-logs: check-docker
celestia-logs:
	@echo "Showing celestia logs"
	$(DOCKER_COMPOSE) logs -f

build:
	cargo build --bins

test-create-token: check-sov-cli
test-create-token:
	$(SOV_CLI_REL_PATH) rpc set-url http://127.0.0.1:12345
	$(SOV_CLI_REL_PATH) keys import --nickname DANGER__DO_NOT_USE_WITH_REAL_MONEY --path ../test-data/keys/token_deployer_private_key.json
	$(SOV_CLI_REL_PATH) transactions import from-file bank --chain-id 0 --path ../test-data/requests/create_token.json
	@echo "Submitting a batch"
	$(SOV_CLI_REL_PATH) rpc submit-batch by-nickname DANGER__DO_NOT_USE_WITH_REAL_MONEY

remove-insecure-keys: check-sov-cli
remove-insecure-keys:
	$(SOV_CLI_REL_PATH) keys remove by-address sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94


fix-readme-toc:
	doctoc README.md --github --notitle
	doctoc README_CELESTIA.md --github --notitle

wait_20:
	sleep 20
