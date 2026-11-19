# project-manager - build and development tasks.
#
# Requires: git 2.30+, Rust (stable), Node.js 22+ and the Tauri prerequisites for your platform
# (https://tauri.app/start/prerequisites/). Run `make help` for the list of targets. See README.md.

.PHONY: help install dev check build test lint fmt clean

help:            ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' | awk -F'\t' '{printf "  make %-10s %s\n", $$1, $$2}'

install:         ## Install frontend dependencies (exact versions from package-lock.json)
	npm --prefix app ci

dev:             ## Run the app in development mode
	npm --prefix app run tauri dev

check:           ## Type-check the frontend and build-check the backend
	npm --prefix app run check
	cd app/src/desktop && cargo check

build:           ## Build release installers for this platform
	npm --prefix app run tauri build

test:            ## Run Rust and frontend tests
	cd app/src/desktop && cargo test
	npm --prefix app test

lint:            ## Run clippy and eslint
	cd app/src/desktop && cargo clippy -- -D warnings
	npm --prefix app run lint

fmt:             ## Format Rust and frontend code
	cd app/src/desktop && cargo fmt
	npm --prefix app run format

clean:           ## Remove build output
	cd app/src/desktop && cargo clean
	rm -rf app/build app/node_modules
