# syntax=docker/dockerfile:1.7
#
# skill-pool web — SvelteKit portal (adapter-node).
#
# Build:
#   docker build -t skill-pool-web:0.1.0 -f web/Dockerfile web/
# Run:
#   docker run --rm -p 3000:3000 \
#     -e ORIGIN=https://skill-pool.example.com \
#     -e SKILL_POOL_API_BASE=http://skill-pool-server \
#     skill-pool-web:0.1.0
#
# Required env:
#   ORIGIN              — adapter-node CSRF origin check; must match the
#                         user-facing scheme://host[:port].
#   SKILL_POOL_API_BASE — where the server lives (set automatically by
#                         the Helm chart to the in-cluster Service URL).
#   PORT                — default 3000.

FROM node:22-alpine AS build
WORKDIR /src

# `npm ci` is reproducible and ~3x faster than `npm install` when a
# lockfile is present. Copy lockfile + manifest first so dep installs
# are cached across rebuilds of the app source.
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY . .
# SvelteKit's adapter-node emits `build/index.js` + `build/` static assets.
RUN npm run build && npm prune --omit=dev

# ----------------------------------------------------------------------
# Runtime — distroless Node, non-root, read-only rootfs friendly.
# ----------------------------------------------------------------------
FROM gcr.io/distroless/nodejs22-debian12 AS runtime
WORKDIR /app

# adapter-node ships the server entrypoint in `build/`. We also copy
# package.json so Node resolves "type":"module" correctly, and the
# pruned node_modules for the tiny set of runtime deps.
COPY --from=build /src/build /app/build
COPY --from=build /src/node_modules /app/node_modules
COPY --from=build /src/package.json /app/package.json

ENV NODE_ENV=production \
    PORT=3000 \
    HOST=0.0.0.0

EXPOSE 3000

LABEL org.opencontainers.image.title="skill-pool-web" \
      org.opencontainers.image.description="SvelteKit portal for the skill-pool registry" \
      org.opencontainers.image.source="https://github.com/olafkfreund/skill_pool" \
      org.opencontainers.image.licenses="MIT"

# Distroless nodejs entrypoint is /usr/bin/node; running with no shell.
USER nonroot
CMD ["/app/build/index.js"]
