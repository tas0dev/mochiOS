.PHONY = build wayland run

build:
	@MOCHI_HOST_POC=1 cargo build --target x86_64-unknown-linux-gnu

run: wayland

wayland: build
	@MOCHI_HOST_POC=1 WAYLAND_DEBUG=1 cargo run --target x86_64-unknown-linux-gnu
