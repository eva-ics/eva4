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

compile-windows:
	rm -rf _build/eva-cs-agent
	mkdir -p ./_build/eva-cs-agent
	ssh -t lab-w1 "cd /src/eva4/svc/controller-system && & 'C:/Program Files/git/bin/git' checkout stable && & 'C:/Program Files/git/bin/git' pull"
	ssh -t lab-w1 "c:/windows/system32/cmd /C c:/src/eva4/svc/controller-system/build-win.bat"
	scp lab-w1:/src/eva4/svc/controller-system/target/release/eva-cs-agent-windows.exe ./_build/eva-cs-agent/eva-cs-agent.exe
	cp agent-config-example.yml ./_build/eva-cs-agent/config.yml

debian-pkg:
	mkdir -p _build
	cd make-deb && TARGET_DIR=target-x86_64-musl PACKAGE=eva-cs-agent ./build.sh && mv *.deb ../_build/

release:
	./release.sh
