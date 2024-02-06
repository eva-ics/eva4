all:
	@echo "Select target"

clean:
	rm -rf _build

bump:
	expr `cat build.number` + 1 > build.number

compile-musl-x86_64:
	CARGO_TARGET_DIR=target-x86_64-musl cross build --target x86_64-unknown-linux-musl --release --features agent-linux,openssl-vendored

compile-musl-aarch64:
	CARGO_TARGET_DIR=target-aarch_64-musl cross build --target aarch64-unknown-linux-musl --release --features agent-linux,openssl-vendored

debian-pkg:
	mkdir _build
	cd make-deb && TARGET_DIR=target-x86_64-musl PACKAGE=eva-cs-agent ./build.sh && mv *.deb ../_build/

release:
	./release.sh
