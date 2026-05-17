.PHONY = build wayland run

build:
	@MOCHI_HOST_POC=1 cargo build --target x86_64-unknown-linux-gnu

run: ui_test
	
ui_test: build
	@VIEWKIT_LAYOUT_DEBUG=1 MOCHI_HOST_POC=1 WAYLAND_DEBUG=1 cargo run --target x86_64-unknown-linux-gnu --bin ui_test

state: build
	@VIEWKIT_LAYOUT_DEBUG=1 MOCHI_HOST_POC=1 WAYLAND_DEBUG=1 cargo run --target x86_64-unknown-linux-gnu --bin stateful_ui