.PHONY: help build build-release test clean run run-paper run-live config-show

help:
	@echo "Polymarket Bot Makefile"
	@echo ""
	@echo "Available targets:"
	@echo "  build         - Build debug version"
	@echo "  build-release - Build release version (optimized)"
	@echo "  test          - Run tests"
	@echo "  clean         - Clean build artifacts"
	@echo "  run           - Run in paper mode"
	@echo "  run-paper     - Run in paper mode (default)"
	@echo "  run-live      - Run in live mode (REAL MONEY)"
	@echo "  config-show   - Show current config"
	@echo ""
	@echo "Available strategies: lead_lag, fair_value, flash_crash, book_imbalance, negrisk_arb, convergence, market_maker"
	@echo ""
	@echo "Examples:"
	@echo "  make run STRATEGIES=lead_lag,fair_value"
	@echo "  make run-live STRATEGIES=lead_lag"

# Default strategies
STRATEGIES ?= lead_lag,fair_value

build:
	cargo build --workspace

build-release:
	cargo build --release --workspace

test:
	cargo test --workspace

clean:
	cargo clean

run run-paper:
	@echo "Starting in paper mode with strategies: $(STRATEGIES)..."
	PMBOT_PRIVATE_KEY=$$PMBOT_PRIVATE_KEY cargo run -- run --strategies $(STRATEGIES)

run-live:
	@echo "Starting in LIVE mode with strategies: $(STRATEGIES)..."
	PMBOT_PRIVATE_KEY=$$PMBOT_PRIVATE_KEY cargo run -- run --config config/default.toml --strategies $(STRATEGIES)

config-show:
	cargo run -- config show
