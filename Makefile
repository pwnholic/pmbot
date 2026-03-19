.PHONY: help build build-release test clean run run-paper run-live run-tui config-show db-view db-setup kill resume dev clippy fmt check run-verbose

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
	@echo "  run-tui       - Run with TUI interface"
	@echo "  run-verbose   - Run with debug logging"
	@echo "  config-show   - Show current config"
	@echo "  db-setup      - Setup database directory"
	@echo "  db-view       - View database contents"
	@echo "  kill          - Create kill switch file"
	@echo "  resume        - Remove kill switch file"
	@echo "  clippy        - Run linter"
	@echo "  fmt           - Format code"
	@echo "  check         - Check code without building"
	@echo ""
	@echo "Available strategies: lead_lag, fair_value, flash_crash, book_imbalance, negrisk_arb, convergence, market_maker"
	@echo ""
	@echo "Examples:"
	@echo "  make run STRATEGIES=lead_lag,fair_value"
	@echo "  make run-live STRATEGIES=lead_lag"
	@echo "  make run-tui STRATEGIES=fair_value"

# Default strategies
STRATEGIES ?= fair_value

build:
	cargo build --workspace

build-release:
	cargo build --release --workspace

test:
	cargo test --workspace

clean:
	cargo clean
	rm -rf data/*.db data/*.db-wal data/*.db-shm

run run-paper:
	@echo "Starting in paper mode with strategies: $(STRATEGIES)..."
	cargo run -- run --paper --strategies $(STRATEGIES)

run-live:
	@echo "Starting in LIVE mode with strategies: $(STRATEGIES)..."
	@test -f .env && set -a && . ./.env && set +a; \
	cargo run -- run --live --strategies $(STRATEGIES)

run-tui:
	@echo "Starting with TUI interface..."
	cargo run -- run --paper --strategies $(STRATEGIES) --tui

run-verbose:
	RUST_LOG=debug cargo run -- run --paper --strategies $(STRATEGIES)

config-show:
	cargo run -- config show

db-setup:
	mkdir -p data

db-view:
	@echo "=== Tables ==="
	@sqlite3 data/pmbot.db ".tables" 2>/dev/null || echo "Database not found"
	@echo "\n=== Recent Trades ==="
	@sqlite3 data/pmbot.db "SELECT * FROM trades ORDER BY timestamp DESC LIMIT10;" 2>/dev/null || echo "No trades yet"
	@echo "\n=== Strategy Metrics ==="
	@sqlite3 data/pmbot.db "SELECT * FROM strategy_metrics;" 2>/dev/null || echo "No metrics yet"

kill:
	@touch /tmp/pmbot-kill
	@echo "Kill switch activated - bot will stop trading"

resume:
	@rm -f /tmp/pmbot-kill
	@echo "Kill switch removed - bot can resume trading"

dev:
	cargo build

clippy:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

check:
	cargo check --workspace
