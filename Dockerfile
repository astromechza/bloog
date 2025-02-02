FROM rust:bookworm AS builder
COPY Cargo.toml Cargo.lock /build/
COPY src /build/src
WORKDIR /build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release

FROM gcr.io/distroless/cc-debian12 AS runner
USER 101:101
COPY --from=builder /build/target/release/bloog /bloog
ENTRYPOINT ["/bloog"]
