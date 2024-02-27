#!/bin/sh

KEEP_NIGHTLY=6
KEEP_STABLE=3

VERSION=$(grep ^version Cargo.toml|cut -d\" -f2)

remove_build() {
  build=$1
  [ -z "${build}" ] && exit 1
  kind=$2
  [ -z "${kind}" ] && exit 1
  echo "Removing build $1 ($2)"
  gsutil -m rm -f "gs://pub.bma.ai/eva4/${VERSION}/${kind}/*${build}*"
}

for build in $(gsutil ls "gs://pub.bma.ai/eva4/${VERSION}/nightly/manifest*"|sort \
  |head -n -${KEEP_NIGHTLY}|sed 's/.*manifest-\([0-9]*\).json/\1/g'); do
  remove_build "$build" nightly
done

for build in $(gsutil ls "gs://pub.bma.ai/eva4/${VERSION}/stable/manifest*"|sort\
  |head -n -${KEEP_STABLE}|sed 's/.*manifest-\([0-9]*\).json/\1/g'); do
  remove_build "$build" stable
done

rci job run pub.bma.ai
echo "Repo cleanup completed"
