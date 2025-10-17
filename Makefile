VERSION=4.1.0

all:
	@echo select target

build: build-increase

build-increase:
	@./dev/increase_build_version

test-build-mark:
	git rev-parse --abbrev-ref HEAD |grep ^main$ > /dev/null
	make build
	git commit -a -m 'test build'
	git push origin main

test-build-create:
	cd /opt/eva-util/eva-builder && ./update.sh lab-builder1
	ssh -t lab-builder1 "cd /build/eva4 && git checkout main && make do-test-build-create"

do-test-build-create:
	git rev-parse --abbrev-ref HEAD |grep ^main$ > /dev/null
	MASTER=allow ./dev/build-and-upload

test-release:
	./dev/make-release --test
	rci job run pub.bma.ai

stable-build-mark:
	git rev-parse --abbrev-ref HEAD |grep ^stable > /dev/null
	make build
	git commit -a -m 'stable build'
	git push

stable-build-create:
	cd /opt/eva-util/eva-builder && ./update.sh lab-builder1
	ssh -t lab-builder1 "cd /build/eva4 && git checkout stable && make do-stable-build-create"

do-stable-build-create:
	git rev-parse --abbrev-ref HEAD |grep ^stable$ > /dev/null
	MASTER=deny ./dev/build-and-upload

stable-release: stable-release-dist stable-release-docker

stable-release-dist:
	git rev-parse --abbrev-ref HEAD |grep ^stable$ > /dev/null
	./dev/make-release
	rci job run pub.bma.ai

stable-release-docker:
	ssh -t lab-builder1 "cd /build/eva4 && git checkout stable && ./dev/make-docker"

release-installer:
	gsutil -h "Cache-Control:no-cache" -h "Content-Type:text/x-shellscript" \
		cp -a public-read install.sh gs://pub.bma.ai/eva4/install
	rci job run pub.bma.ai

release-switch-arch:
	gsutil -h "Cache-Control:no-cache" -h "Content-Type:text/x-shellscript" \
		cp -a public-read ./tools/switch-arch gs://pub.bma.ai/eva4/tools/switch-arch
	rci job run pub.bma.ai

ver: build-increase update-version

update-version:
	find eva svc tools /opt/eva4-enterprise/eva4-esvc -name Cargo.toml -exec sed -i '' "s/^version = .*/version = \"${VERSION}\"/g" {} \;
	sed -i '' "s/^VERSION=.*/VERSION=${VERSION}/g" update.sh
	sed -i '' "s/^version = .*/version = \"${VERSION}\"/g" Cargo.toml

repo-cleanup:
	@./dev/repo-cleanup.sh
