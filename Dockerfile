FROM rust:bookworm AS builder
COPY Cargo.toml Cargo.lock /build/
COPY src /build/src
WORKDIR /build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release

FROM debian:bookworm-slim AS runner
USER nobody:nogroup
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && apt-get clean
COPY --from=builder /build/target/release/bloog /bloog
ENTRYPOINT ["/bloog"]
