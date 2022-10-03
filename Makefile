VERSION=4.0.0

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
	ssh -t lab-builder1 "cd /build/eva4 && make do-test-build-create"

do-test-build-create:
	git rev-parse --abbrev-ref HEAD |grep ^main$ > /dev/null
	MASTER=allow ./dev/build-and-upload

test-release:
	git rev-parse --abbrev-ref HEAD |grep ^main$ > /dev/null
	./dev/make-release --test
	jks build pub.bma.ai

stable-build-mark:
	git rev-parse --abbrev-ref HEAD |grep ^4 > /dev/null
	make build
	git commit -a -m 'stable build'
	git push

stable-build-create:
	git rev-parse --abbrev-ref HEAD |grep ^4 > /dev/null
	MASTER=deny ./dev/build-and-upload

stable-release:
	git rev-parse --abbrev-ref HEAD |grep ^4 > /dev/null
	./dev/make-release
	jks build pub.bma.ai

release-installer:
	gsutil -h "Cache-Control:no-cache" -h "Content-Type:text/x-shellscript" \
		cp -a public-read install.sh gs://pub.bma.ai/eva4/install
	jks build pub.bma.ai

ver: build-increase update-version

update-version:
	find eva svc tools -name Cargo.toml -exec sed -i "s/^version = .*/version = \"${VERSION}\"/g" {} \;
	sed -i "s/^VERSION=.*/VERSION=${VERSION}/g" update.sh
	sed -i "s/^__version__ =.*/__version__ = '${VERSION}'/g" /opt/eva-doc/doc/conf.py
