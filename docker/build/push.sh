#!/bin/sh -x

BUILD=$1

if [ -z $BUILD ]; then
  echo "Usage $0 <BUILD>"
  exit 9
fi

docker tag altertech/eva-ics4:latest "altertech/eva-ics4:${BUILD}" || exit 1
docker push "altertech/eva-ics4:$BUILD" || exit 1
docker push altertech/eva-ics4:latest || exit 1
