#!/bin/bash

# L-SAMP 100 | Tauri Migration Quick Start
# This script helps you set up and run the project in Tauri dev mode

set -e

echo "🎵 L-SAMP 100 | Tauri Migration Quick Start"
echo "============================================="
echo ""

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    echo "❌ Node.js is not installed"
    exit 1
fi

# Check if Rust is installed
if ! command -v cargo &> /dev/null; then
    echo "⚠️  Rust/Cargo is not installed"
    echo "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source $HOME/.cargo/env
fi

echo "✅ Prerequisites found"
echo ""

# Install npm dependencies
echo "📦 Installing npm dependencies..."
npm install

echo ""
echo "✅ Setup complete!"
echo ""
echo "Next steps:"
echo "1. Open two terminal windows"
echo "2. In Terminal 1, run: npm start"
echo "3. Wait for Angular dev server to start (http://localhost:4200)"
echo "4. In Terminal 2, run: npm run tauri-dev"
echo ""
echo "The Tauri window should open and connect to your Angular app."
echo ""
echo "Happy coding! 🚀"
