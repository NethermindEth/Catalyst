DATABASE_URL ?= mysql://root:password@127.0.0.1:3306/urc
RUST_LOG ?= info
REGISTRY_ADDRESS=0xFd14bc4D6d794a9197D550C10804aA77990059F7
L1_RPC_URL ?= http://localhost:8545

DOCKER_COMPOSE ?= docker compose

.PHONY: db-up db-down db-logs run-monitor

db-up:
	$(DOCKER_COMPOSE) up -d mysql

db-down:
	$(DOCKER_COMPOSE) down

db-logs:
	$(DOCKER_COMPOSE) logs -f mysql

run-monitor:
	L1_RPC_URL=$(L1_RPC_URL) DATABASE_URL=$(DATABASE_URL) REGISTRY_ADDRESS=$(REGISTRY_ADDRESS) cargo run --bin urc-monitor
