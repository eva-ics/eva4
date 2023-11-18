#!/bin/sh

VERSION=$(grep "^#define EVA4_CCP_SDK_VERSION" ffi-sdk/eva4-ffi-sdk.hpp|awk '{ print $3 }'|tr -d '"')
BUILD_DIR="/tmp/eva4-ffi-sdk-cpp-${VERSION}"
FILE="eva4-ffi-sdk-cpp-${VERSION}.zip"

URI="pub.bma.ai/eva4/sdk/cpp/${FILE}"

if ! curl -sI "https://$URI" 2>&1|grep " 404 " > /dev/null; then
  echo "The version is already published"
  exit 1
fi

rm -rf "${BUILD_DIR}" || exit 2
mkdir -p "${BUILD_DIR}" || exit 2
cp -rv ./ffi-sdk/* "${BUILD_DIR}" || exit 2
cp -rv ../common/*.h "${BUILD_DIR}" || exit 2
cd "${BUILD_DIR}" || exit 2
cd .. || exit 2

rm -f "${FILE}" || exit 3
zip -r "${FILE}" "eva4-ffi-sdk-cpp-${VERSION}" || exit 3
rm -rf "${BUILD_DIR}" || exit 3

echo "${FILE} created"

gsutil cp -a public-read "${FILE}" "gs://${URI}" || exit 1

rm -f "${FILE}"

rci job run pub.bma.ai

echo

echo "Released: ${VERSION}"
