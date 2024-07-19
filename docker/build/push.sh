#!/bin/sh -x

BUILD=$1

if [ -z $BUILD ]; then
  echo "Usage $0 <BUILD>"
  exit 9
fi

docker tag bmauto/eva-ics4:latest "bmauto/eva-ics4:${BUILD}" || exit 1
docker push "bmauto/eva-ics4:$BUILD" || exit 1
docker push bmauto/eva-ics4:latest || exit 1
