FROM rust:1.86-bookworm AS chef
RUN apt-get update -q && \
    apt-get upgrade -y --no-install-recommends && \
    apt-get install -y --no-install-recommends \
        gcc \
        pkg-config \
        openssl \
        libssl-dev \
        ca-certificates && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb /var/cache/apt/*.bin
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
RUN cargo build --release --bin telepirate && \
    rm -rf target/release/deps target/release/build

FROM debian:bookworm-slim AS telepirate
RUN apt-get update -q && \
    apt-get upgrade -y --no-install-recommends && \
    apt-get install -y --no-install-recommends \
        python3 \
        python3-pip \
        ffmpeg \
        ca-certificates && \
    pip install --no-cache-dir --break-system-packages -U "yt-dlp[default]" && \
    apt-get autoremove -y && \
    apt-get clean && \
    rm -rf \
        /var/lib/apt/lists/* \
        /var/cache/apt/archives/*.deb \
        /var/cache/apt/*.bin \
        /root/.cache/pip

COPY --from=builder /app/target/release/telepirate /usr/local/bin/

ENTRYPOINT ["telepirate"]
