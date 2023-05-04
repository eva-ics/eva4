#!/bin/sh

VERSION=$1
BUILD=$2

if [ -z $BUILD ]; then
  echo "Usage $0 <VERSION> <BUILD>"
  exit 9
fi

ARGS="--no-cache"
[ "$TEST" = "1" ] && ARGS=
sed "s/%VERSION%/$VERSION/g" Dockerfile.in | sed "s/%BUILD%/$BUILD/g" > Dockerfile || exit 1
docker build $ARGS -t altertech/eva-ics4 . || exit 1
echo "Image build completed (${VERSION} $BUILD)"
