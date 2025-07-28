FROM rust:1.88-alpine AS chef
# Default build profile is dev
ARG BUILD_PROFILE=dev
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
RUN cargo chef cook --profile ${BUILD_PROFILE} --locked --recipe-path recipe.json
COPY . .
RUN cargo install --profile ${BUILD_PROFILE} --locked --path .

FROM alpine:edge AS runtime
RUN apk add --no-cache \
    ffmpeg \
    imagemagick \
    jpegoptim \
    ca-certificates \
    yt-dlp
COPY --from=builder /usr/local/cargo/bin/telepirate /usr/bin/
ENTRYPOINT [ "telepirate" ]
