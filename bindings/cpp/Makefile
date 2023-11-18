all:
	@echo "select target"

pub:
	./release.sh

doc:
	rm -rf docs
	doxygen Doxyfile.in
	cd docs/html && gsutil -m rsync -r -d -c -a public-read -r . gs://pub.bma.ai/dev/docs/eva4-cpp-sdk/
