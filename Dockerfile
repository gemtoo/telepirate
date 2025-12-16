FROM rust:1.92-alpine AS chef
# Default build profile is dev
ARG BUILD_PROFILE=dev
RUN apk add --no-cache \
    build-base \
    pkgconfig \
    openssl-dev \
    openssl-libs-static \
    musl-dev \
    ca-certificates || true
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
ARG S6_OVERLAY_VERSION=3.2.1.0
# Detect architecture and set S6_ARCH accordingly
RUN S6_ARCH=$(uname -m) && \
    case "${S6_ARCH}" in \
        x86_64) S6_ARCH=x86_64 ;; \
        aarch64) S6_ARCH=aarch64 ;; \
        armv7l|armv6l) S6_ARCH=arm ;; \
        riscv64) S6_ARCH=riscv64 ;; \
        *) echo "Unsupported architecture: ${S6_ARCH}"; exit 1 ;; \
    esac && \
    echo "Detected architecture: ${S6_ARCH}" && \
    wget -O /tmp/s6-overlay-noarch.tar.xz "https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-noarch.tar.xz" && \
    wget -O /tmp/s6-overlay-arch.tar.xz "https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-${S6_ARCH}.tar.xz" && \
    wget -O /tmp/s6-overlay-symlinks.tar.xz "https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-symlinks-noarch.tar.xz" && \
    tar -C / -Jxpf /tmp/s6-overlay-noarch.tar.xz && \
    tar -C / -Jxpf /tmp/s6-overlay-arch.tar.xz && \
    tar -C / -Jxpf /tmp/s6-overlay-symlinks.tar.xz && \
    rm /tmp/s6-overlay-*.tar.xz
RUN apk upgrade || true
RUN apk add --no-cache \
    bash \
    ffmpeg \
    imagemagick \
    jpegoptim \
    ca-certificates \
    deno \
    py3-pip || true
RUN python3 -m pip install --break-system-packages -U "yt-dlp[default]" --root-user-action ignore
# Check if crond is present in default Alpine, as it might change
RUN command -v crond
# Bash is needed as the default shell in s6-overlay
RUN ln -sf /bin/bash /bin/sh
RUN echo '0 */6 * * * /usr/bin/python3 -m pip install --break-system-packages -U "yt-dlp[default]" --root-user-action ignore' > /etc/crontabs/root
COPY --chown=root:root --chmod=755 services.d /etc/services.d
COPY --chown=root:root --chmod=755 cont-init.d /etc/cont-init.d
COPY --from=builder /usr/local/cargo/bin/telepirate /usr/bin/
ENTRYPOINT [ "/init" ]
