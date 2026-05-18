# syntax=docker/dockerfile:1.7
FROM rust:1.94-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    libxml2-dev \
    libxmlsec1-dev \
    libxmlsec1-openssl \
    libtool-bin \
    libltdl-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache deps
COPY Cargo.toml Cargo.lock* ./
COPY server/Cargo.toml server/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
RUN mkdir -p server/src cli/src \
    && echo 'fn main() {}' > server/src/main.rs \
    && echo 'fn main() {}' > cli/src/main.rs \
    && cargo build --release --bin skill-pool-server \
    && rm -rf server/src cli/src target/release/deps/skill_pool*

# Real build
COPY server server
COPY cli cli
RUN cargo build --release --bin skill-pool-server

FROM debian:bookworm-slim AS runtime
# Slim is a debian image (not distroless) because we need the xmlsec1
# shared libs at runtime for SAML signature validation. Trade-off: ~20 MB
# bigger than distroless, but it's still <80 MB total.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libxml2 \
    libxmlsec1 \
    libxmlsec1-openssl \
    libltdl7 \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && adduser --system --group --no-create-home skillpool
WORKDIR /app
COPY --from=build /src/target/release/skill-pool-server /usr/local/bin/skill-pool-server
COPY server/migrations /app/migrations
ENV SKILL_POOL_BIND=0.0.0.0:8080
EXPOSE 8080
USER skillpool
ENTRYPOINT ["/usr/local/bin/skill-pool-server"]
