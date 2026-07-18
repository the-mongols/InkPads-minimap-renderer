#!/bin/bash

echo "========================================"
echo "  Building Minimap Renderer (Release)"
echo "========================================"

# Check if cargo is installed
if ! command -v cargo &> /dev/null
then
    echo "[ERROR] 'cargo' (Rust toolchain) not found. Please install Rust from https://rustup.rs."
    exit 1
fi

echo "[1/2] Compiling Rust renderer in release mode..."
cargo build --release --bin minimap_renderer --features bin,vfs,vulkan,cpu --no-default-features

if [ $? -ne 0 ]; then
    echo "[ERROR] Compilation failed."
    exit 1
fi

echo "[2/2] Verifying release binary..."
if [ -f target/release/minimap_renderer ]; then
    echo "[SUCCESS] Renderer built successfully at target/release/minimap_renderer"
elif [ -f target/release/minimap_renderer.exe ]; then
    echo "[SUCCESS] Renderer built successfully at target/release/minimap_renderer.exe"
else
    echo "[WARNING] Binary not found at standard release path."
fi

echo "========================================"
echo "  Build Complete!"
echo "========================================"
