VERSION=0.2.34

all:
	@echo "what do you want to build today?"

ver:
	find . -type f -name "*.py" ! -name "__main__.py" -exec sed -i "s/^__version__ = .*/__version__ = '${VERSION}'/g" {} \;

d: build

build:
	rm -rf dist build evaics.egg-info
	python3 setup.py sdist

pub: d pub-pypi pub-anaconda

pub-pypi: upload-pypi

pub-anaconda:
	/opt/anaconda3/bin/anaconda upload -u bohemia-automation dist/*.tar.gz

upload-pypi:
	twine upload dist/*

clean:
	rm -rf dist build evaics.egg-info
