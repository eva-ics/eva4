all:
  @echo "Select target"

bump:
	npm version --no-git-tag-version patch

pub: upload

upload:
	npm publish --access public
