#!/bin/bash -xe

AGENT=..

VERSION=$(grep ^version ${AGENT}/Cargo.toml|awk '{ print $3 }'|tr -d '"')
BUILD=$(cat ${AGENT}/build.number)
DESCRIPTION=$(grep "^const DESCRIPTION" ${AGENT}/src/agent/linux.rs | awk -F\" '{ print $2 }')

[ -z "$VERSION" ] && exit 1
[ -z "$BUILD" ] && exit 1
[ -z "$DESCRIPTION" ] && exit 1

TARGET="${PACKAGE}-${VERSION}-${BUILD}-amd64${PACKAGE_SUFFIX}"
[ -z "${RUST_TARGET}" ] && RUST_TARGET=x86_64-unknown-linux-gnu
[ -z "${TARGET_DIR}" ] && TARGET_DIR=target

rm -rf "./${TARGET}"
mkdir -p "./${TARGET}/usr/sbin"
mkdir -p "./${TARGET}/lib/systemd/system"
mkdir -p "./${TARGET}/DEBIAN"
cp -vf "${AGENT}/${TARGET_DIR}/${RUST_TARGET}/release/eva-cs-agent-linux" "./${TARGET}/usr/sbin/eva-cs-agent"
cp -vf "${AGENT}/eva-cs-agent.service" "./${TARGET}/lib/systemd/system/"
mkdir -p "./${TARGET}/etc/eva-cs-agent"
cp "${AGENT}/agent-config-example.yml" "./${TARGET}/etc/eva-cs-agent/config.yml-dist"
(
cat << EOF
Package: ${PACKAGE}
Version: ${VERSION}-${BUILD}
Section: base
Priority: optional
Architecture: amd64
Maintainer: Serhij S. <div@altertech.com>
Description: ${DESCRIPTION}
EOF
) > "./${TARGET}/DEBIAN/control"
cp -vf ./debian/* "./${TARGET}/DEBIAN/"
dpkg-deb --build "./${TARGET}"
