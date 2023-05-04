#!/bin/sh

BUILD=$2

if [ -z $BUILD ]; then
  echo "Usage $0 <BUILD>"
  exit 9
fi

docket tag "altertech/eva-ics4:${BUILD}" altertech/eva-ics4:latest || exit 1
docker push "altertech/eva-ics:$BUILD" || exit 1
docker push altertech/eva-ics:latest || exit 1
