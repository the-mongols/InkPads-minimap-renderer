@echo off
echo ========================================
echo   Building Minimap Renderer (Release)
echo ========================================

cargo --version >nul 2>nul
if errorlevel 1 goto no_cargo

echo [1/2] Compiling Rust renderer in release mode...
cargo build --release --bin minimap_renderer --features bin,vfs,vulkan,cpu --no-default-features
if errorlevel 1 goto build_failed

echo [2/2] Verifying release binary...
if exist target\release\minimap_renderer.exe goto got_exe
if exist target\release\minimap_renderer goto got_elf
goto no_binary

:got_exe
echo [SUCCESS] Renderer built successfully at target\release\minimap_renderer.exe
echo [3/3] Copying binary to bot folders...
copy /y target\release\minimap_renderer.exe inkpads-bot\minimap_renderer.exe >nul
copy /y target\release\minimap_renderer.exe ..\InkPads\packages\renderer\inkpads-bot\minimap_renderer.exe >nul
if errorlevel 1 (
    echo [WARNING] Failed to copy binary to bot folders.
) else (
    echo [SUCCESS] Copied target\release\minimap_renderer.exe to bot folders.
)
goto done

:got_elf
echo [SUCCESS] Renderer built successfully at target\release\minimap_renderer
echo [3/3] Copying binary to inkpads-bot folder...
copy /y target\release\minimap_renderer inkpads-bot\minimap_renderer >nul
if errorlevel 1 (
    echo [WARNING] Failed to copy binary to inkpads-bot folder.
) else (
    echo [SUCCESS] Copied target\release\minimap_renderer to inkpads-bot\minimap_renderer
)
goto done

:no_cargo
echo [ERROR] 'cargo' (Rust toolchain) not found. Please install Rust from https://rustup.rs.
exit /b 1

:build_failed
echo [ERROR] Compilation failed.
exit /b 1

:no_binary
echo [WARNING] Binary not found at standard release path.
goto done

:done
echo ========================================
echo   Build Complete!
echo ========================================
