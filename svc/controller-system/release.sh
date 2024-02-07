#!/bin/sh

VERSION=$(grep ^version ./Cargo.toml|awk '{ print $3 }'|tr -d '"')
BUILD=$(cat ./build.number)

URI=pub.bma.ai/eva-cs-agent


[ -z "$VERSION" ] && exit 1
[ -z "$BUILD" ] && exit 1

LINUX_BINARY_X86_64=eva-cs-agent-linux-${VERSION}-${BUILD}-x86_64
LINUX_BINARY_AARCH64=eva-cs-agent-linux-${VERSION}-${BUILD}-aarch64
DEBIAN_PACKAGE_X86_64=eva-cs-agent-4.0.2-1-amd64.deb
WINDOWS_PACKAGE_X86_64=eva-cs-agent-windows-x86_64-${VERSION}-${BUILD}.zip

( curl -sI "https://${URI}/${LINUX_BINARY_X86_64}" 2>&1|head -1|grep " 404 " ) > /dev/null 2>&1

if [ $? -ne 0 ]; then
  echo "Build ${BUILD} already released"
  exit 2
fi

make clean || exit 3
make compile-musl-x86_64 || exit 3
make compile-musl-aarch64 || exit 3
make compile-windows || exit 3

cd _build || exit 3
zip -r "$WINDOWS_PACKAGE_X86_64" eva-cs-agent || exit 3
cd .. || exit 3

make debian-pkg || exit 3

gsutil cp -a public-read \
  ./target-x86_64-musl/x86_64-unknown-linux-musl/release/eva-cs-agent-linux \
  "gs://${URI}/${LINUX_BINARY_X86_64}" || exit 4

gsutil cp -a public-read \
  ./target-aarch_64-musl/aarch64-unknown-linux-musl/release/eva-cs-agent-linux \
  "gs://${URI}/${LINUX_BINARY_AARCH64}" || exit 4

gsutil cp -a public-read \
  "./_build/${DEBIAN_PACKAGE_X86_64}" \
  "gs://${URI}/${DEBIAN_PACKAGE_X86_64}" || exit 4

gsutil cp -a public-read \
  "./_build/${WINDOWS_PACKAGE_X86_64}" \
  "gs://${URI}/${WINDOWS_PACKAGE_X86_64}" || exit 4

rci job run pub.bma.ai || exit 5

echo ""
echo "Release successful"
