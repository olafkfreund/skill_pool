# syntax=docker/dockerfile:1.7
FROM rust:1.94-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

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

FROM gcr.io/distroless/cc-debian12 AS runtime
WORKDIR /app
COPY --from=build /src/target/release/skill-pool-server /usr/local/bin/skill-pool-server
COPY server/migrations /app/migrations
ENV SKILL_POOL_BIND=0.0.0.0:8080
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/skill-pool-server"]
