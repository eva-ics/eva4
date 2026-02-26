## syntax=docker/dockerfile:1.7
ARG BASE_IMAGE=timescale/timescaledb:2.25.1-pg18
ARG PGRX_VERSION=0.17.0

FROM ${BASE_IMAGE} AS builder

ARG PGRX_VERSION

ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:${PATH} \
    RUSTFLAGS="-C target-feature=-crt-static" \
    PGRX_HOME=/usr/local/pgrx \
    LIBCLANG_PATH=/usr/lib

RUN apk add --no-cache \
    bash \
    build-base \
    ca-certificates \
    clang \
    clang19-libclang \
    curl \
    git \
    llvm-dev \
    openssl-dev \
    pkgconf \
    postgresql18-dev \
    rustup \
    zlib-dev

RUN rustup-init -y --profile minimal --default-toolchain stable \
    && rustup component add rustfmt clippy

RUN cargo install --locked cargo-pgrx --version ${PGRX_VERSION}
RUN mkdir -p "${PGRX_HOME}" \
    && cargo pgrx init --pg18 "$(command -v pg_config)"

WORKDIR /work
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=eva_pg_cargo_registry \
    cargo pgrx package \
    --pg-config "$(command -v pg_config)" \
    --features pg18

FROM ${BASE_IMAGE} AS runtime

COPY --from=builder /work/target/release/eva_pg-pg18/usr/local/ /usr/local/
