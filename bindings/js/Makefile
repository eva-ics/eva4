all:
	npm run build

bump:
	npm version --no-git-tag-version patch

pub: all upload doc

upload:
	cd dist && npm publish --access public

doc:
	rm -rf docs
	typedoc
	cd docs && gsutil -m cp -a public-read -r . gs://pub.bma.ai/dev/docs/eva4-js-sdk/
