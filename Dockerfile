# syntax=docker/dockerfile:1
# Discord bot (outbound only, no exposed port). Stage 1 builds the Linux
# renderer + replayshark; stage 2 runs the bot with those binaries.

########################  Stage 1: build the Rust binaries  ####################
# Base matches rust-toolchain.toml (1.92.0).
FROM rust:1.92-bookworm AS build

# nasm: openh264 CPU encoder SIMD. cmake/clang/build-essential/pkg-config/libssl-dev: -sys deps.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential nasm pkg-config libssl-dev cmake clang \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin minimap_renderer --no-default-features --features bin,vfs,vulkan,cpu \
 && cargo build --release --bin replayshark \
 && mkdir -p /out \
 && cp target/release/minimap_renderer target/release/replayshark /out/

########################  Stage 2: python bot runtime  #########################
FROM python:3.12-slim-bookworm AS runtime

# vulkan libs: software Vulkan (lavapipe) lets device init succeed on a GPU-less host.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ffmpeg ca-certificates procps libssl3 \
        fontconfig fonts-dejavu-core \
        libvulkan1 mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 appuser
WORKDIR /app

COPY inkpads-bot/requirements.txt ./inkpads-bot/requirements.txt
RUN pip install --no-cache-dir -r inkpads-bot/requirements.txt

# Committed *.exe are Windows-only; drop in the Linux binaries and delete them.
COPY inkpads-bot/ ./inkpads-bot/
COPY --from=build /out/minimap_renderer ./inkpads-bot/minimap_renderer
COPY --from=build /out/replayshark      ./inkpads-bot/replayshark
RUN chmod +x ./inkpads-bot/minimap_renderer ./inkpads-bot/replayshark \
 && rm -f ./inkpads-bot/*.exe ./inkpads-bot/*.pdb \
 && mkdir -p ./inkpads-bot/temp \
 && chown -R appuser:appuser /app

# Host policy (FORCE_CPU / RENDERER_CODEC / DISCORD_TOKEN) comes from .env.
ENV RENDERER_PATH=/app/inkpads-bot/minimap_renderer \
    REPLAYSHARK_EXE=/app/inkpads-bot/replayshark \
    WOWS_EXTRACTED_DIR=/data/game_data/extracted \
    PYTHONUNBUFFERED=1

USER appuser
WORKDIR /app/inkpads-bot

# Process healthcheck (no port to probe).
HEALTHCHECK --interval=60s --timeout=10s --start-period=40s --retries=3 \
    CMD pgrep -f bot.py >/dev/null || exit 1

CMD ["python", "bot.py"]
