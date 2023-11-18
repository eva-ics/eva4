all:
	@echo "select target"

pub:
	./release.sh

doc:
	rm -rf docs
	doxygen Doxyfile.in
	cd docs/html && gsutil -m cp -a public-read -r . gs://pub.bma.ai/dev/docs/eva4-cpp-sdk/
