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
# Install s6-overlay init system
ARG S6_OVERLAY_VERSION=3.2.1.0
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-noarch.tar.xz /tmp
RUN tar -C / -Jxpf /tmp/s6-overlay-noarch.tar.xz
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-x86_64.tar.xz /tmp
RUN tar -C / -Jxpf /tmp/s6-overlay-x86_64.tar.xz
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-symlinks-noarch.tar.xz /tmp
RUN tar -C / -Jxpf /tmp/s6-overlay-symlinks-noarch.tar.xz
# Check if crond is present in default Alpine, as it might change
RUN apk add --no-cache \
    ffmpeg \
    imagemagick \
    jpegoptim \
    ca-certificates \
    yt-dlp
RUN echo '0 */6 * * * /sbin/apk upgrade' > /etc/crontabs/root
COPY --chown=root:root --chmod=755 services.d /etc/services.d
COPY --chown=root:root --chmod=755 cont-init.d /etc/cont-init.d
COPY --from=builder /usr/local/cargo/bin/telepirate /usr/bin/
ENTRYPOINT [ "/init" ]
