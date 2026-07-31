#!/usr/bin/env pwsh
# scripts/build-desktop.ps1
# ─────────────────────────────────────────
# One-shot build of the OPC DA Desktop GUI.
#
# Pipeline:
#   1. `npm install` + `npm run build`  →  opc-da-desktop/ui/dist/
#   2. `cargo build --release -p opc-da-desktop`  →  target/release/opc-da-desktop.exe
#
# Frontend assets are embedded into the exe at compile time
# (`tauri::generate_context!`), so the final exe is a self-contained
# distribution — no Node, no Vite, no source files required to run.
#
# Usage:
#   pwsh -File scripts/build-desktop.ps1                  # 64-bit (default)
#   pwsh -File scripts/build-desktop.ps1 -Arch x86        # 32-bit
#   pwsh -File scripts/build-desktop.ps1 -Run             # build then launch
#   pwsh -File scripts/build-desktop.ps1 -SkipUi          # cargo-only (reuse existing dist/)
#   pwsh -File scripts/build-desktop.ps1 -Arch x86 -Run   # 32-bit then launch

[CmdletBinding()]
param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [switch]$Run,
    [switch]$SkipUi,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

$dist = "opc-da-desktop/ui/dist"
# `cargo build --release` without `--target` builds for the host (64-bit) and
# outputs to target/release/; the 32-bit build targets i686 explicitly.
$archLabel = if ($Arch -eq "x86") { "32-bit" } else { "64-bit" }
$rustTarget = if ($Arch -eq "x86") { "i686-pc-windows-msvc" } else { $null }
$exeDir = if ($Arch -eq "x86") { "target/i686-pc-windows-msvc/release" } else { "target/release" }
$exe = "$exeDir/opc-da-desktop.exe"

if ($Clean) {
    Write-Host "Cleaning previous build artifacts..." -ForegroundColor Yellow
    Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
    if ($rustTarget) {
        cargo clean -p opc-da-desktop --target $rustTarget 2>&1 | Out-Null
    } else {
        cargo clean -p opc-da-desktop 2>&1 | Out-Null
    }
}

if (-not $SkipUi) {
    Write-Host "==> [1/2] Building frontend (npm install + vite build)" -ForegroundColor Cyan
    Push-Location opc-da-desktop/ui
    if (-not (Test-Path "node_modules")) {
        npm install
        if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
    }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build failed" }
    Pop-Location
} else {
    Write-Host "==> [1/2] Skipping frontend (using existing $dist)" -ForegroundColor DarkGray
}

# Ensure the 32-bit Rust target is installed before building it.
if ($rustTarget -and -not (rustup target list --installed | Select-String -SimpleMatch -Quiet $rustTarget)) {
    Write-Host "Rust target $rustTarget not installed - adding..." -ForegroundColor Yellow
    rustup target add $rustTarget
    if ($LASTEXITCODE -ne 0) { throw "rustup target add $rustTarget failed" }
}

Write-Host "==> [2/2] Building ${archLabel} release exe (frontend assets will be embedded)" -ForegroundColor Cyan
if ($rustTarget) {
    cargo build --release -p opc-da-desktop --target $rustTarget
} else {
    cargo build --release -p opc-da-desktop
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exeInfo = Get-Item $exe -ErrorAction SilentlyContinue
if (-not $exeInfo) {
    throw "Expected $exe not found after build"
}

Write-Host ""
Write-Host "✓ Built $exe ($([math]::Round($exeInfo.Length / 1MB, 2)) MB, ${archLabel})" -ForegroundColor Green
Write-Host "  Double-click to launch — no installation required." -ForegroundColor Green
Write-Host ""

if ($Run) {
    Write-Host "Launching $exe ..." -ForegroundColor Cyan
    & $exeInfo.FullName
}