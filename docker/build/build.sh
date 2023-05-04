#!/bin/sh

VERSION=$1
BUILD=$2

if [ -z $BUILD ]; then
  echo "Usage $0 <VERSION> <BUILD>"
  exit 9
fi

sed "s/%VERSION%/$VERSION/g" Dockerfile.in | sed "s/%BUILD%/$BUILD/g" > Dockerfile || exit 1
docker build -t altertech/eva-ics4 . || exit 1
echo "Image build completed (${VERSION} $BUILD)"
