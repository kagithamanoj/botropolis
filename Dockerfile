# Botropolis, hosted: the Rust server in a slim image.
#
# Build stage: the stable toolchain compiles the workspace.
# Runtime stage: the release binary, the static web UI, the agent
# specs, and the model registry. State lives in a volume at
# /app/botropolis/data (see docs/hosting.md).

FROM rust:1-bookworm AS build
WORKDIR /app
COPY rust/Cargo.toml rust/Cargo.lock ./rust/
COPY rust/botropolis ./rust/botropolis
RUN cargo build --release --manifest-path rust/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m botropolis \
    && mkdir -p /app/botropolis/data \
    && chown -R botropolis:botropolis /app
WORKDIR /app
COPY --from=build /app/rust/target/release/botropolis /usr/local/bin/botropolis
COPY botropolis/agents ./botropolis/agents
COPY botropolis/web ./botropolis/web
COPY models ./models
USER botropolis
ENV BOTROPOLIS_HOSTED=1
ENV PORT=8000
VOLUME ["/app/botropolis/data"]
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD curl -f http://127.0.0.1:${PORT:-8000}/health || exit 1
CMD ["sh", "-c", "botropolis serve --host 0.0.0.0 --port ${PORT:-8000} --hosted"]
