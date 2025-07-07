FROM rust:1.88-alpine AS chef
RUN apk add --no-cache \
    build-base \
    pkgconfig \
    openssl-dev \
    openssl-libs-static \
    musl-dev \
    ca-certificates
RUN cargo install cargo-chef --locked

FROM chef AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin telepirate

FROM alpine:edge AS runtime
RUN apk add --no-cache \
    ffmpeg \
    ca-certificates \
    yt-dlp
COPY --from=builder /app/target/release/telepirate /usr/bin/
ENTRYPOINT [ "telepirate" ]
