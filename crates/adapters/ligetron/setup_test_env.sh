#!/bin/bash

# Setup script for Ligetron adapter testing
# This script helps configure the test environment with your Ligetron binaries

set -e

echo "🔧 Setting up Ligetron adapter test environment..."

# Function to check if a file exists and is executable
check_executable() {
    local file="$1"
    local name="$2"
    
    if [ ! -f "$file" ]; then
        echo "❌ $name not found: $file"
        return 1
    fi
    
    if [ ! -x "$file" ]; then
        echo "❌ $name is not executable: $file"
        echo "   Try: chmod +x $file"
        return 1
    fi
    
    echo "✅ $name found and executable: $file"
    return 0
}

# Check if binaries exist in test_binaries directory
if [ -f "./test_binaries/webgpu_prover" ] && [ -f "./test_binaries/webgpu_verifier" ]; then
    echo "📁 Found binaries in test_binaries directory"
    
    PROVER_PATH="$(pwd)/test_binaries/webgpu_prover"
    VERIFIER_PATH="$(pwd)/test_binaries/webgpu_verifier"
    SHADER_PATH="$(pwd)/test_binaries/shader"
    
    check_executable "$PROVER_PATH" "Prover" || exit 1
    check_executable "$VERIFIER_PATH" "Verifier" || exit 1
    
    if [ ! -d "$SHADER_PATH" ]; then
        echo "⚠️  Shader directory not found: $SHADER_PATH"
        echo "   Creating empty shader directory for testing..."
        mkdir -p "$SHADER_PATH"
    fi
    
    export LIGETRON_PROVER="$PROVER_PATH"
    export LIGETRON_VERIFIER="$VERIFIER_PATH"
    export LIGETRON_SHADER_PATH="$SHADER_PATH"
    
elif [ -n "$LIGETRON_PROVER" ] && [ -n "$LIGETRON_VERIFIER" ]; then
    echo "🌍 Using environment variables for binary paths"
    
    check_executable "$LIGETRON_PROVER" "Prover (from env)" || exit 1
    check_executable "$LIGETRON_VERIFIER" "Verifier (from env)" || exit 1
    
    if [ -z "$LIGETRON_SHADER_PATH" ]; then
        echo "⚠️  LIGETRON_SHADER_PATH not set, using default: ./shader"
        export LIGETRON_SHADER_PATH="./shader"
        mkdir -p "$LIGETRON_SHADER_PATH"
    fi
    
elif command -v webgpu_prover >/dev/null 2>&1 && command -v webgpu_verifier >/dev/null 2>&1; then
    echo "🔍 Found binaries in PATH"
    
    PROVER_PATH=$(which webgpu_prover)
    VERIFIER_PATH=$(which webgpu_verifier)
    
    check_executable "$PROVER_PATH" "Prover (from PATH)" || exit 1
    check_executable "$VERIFIER_PATH" "Verifier (from PATH)" || exit 1
    
    export LIGETRON_PROVER="$PROVER_PATH"
    export LIGETRON_VERIFIER="$VERIFIER_PATH"
    export LIGETRON_SHADER_PATH="${LIGETRON_SHADER_PATH:-./shader}"
    mkdir -p "$LIGETRON_SHADER_PATH"
    
else
    echo "❌ Ligetron binaries not found!"
    echo ""
    echo "Please choose one of these options:"
    echo ""
    echo "📁 Option 1: Copy binaries to test_binaries directory"
    echo "   mkdir -p test_binaries"
    echo "   cp /path/to/your/webgpu_prover test_binaries/"
    echo "   cp /path/to/your/webgpu_verifier test_binaries/"
    echo "   cp -r /path/to/your/shader test_binaries/"
    echo ""
    echo "🌍 Option 2: Set environment variables"
    echo "   export LIGETRON_PROVER=/path/to/your/webgpu_prover"
    echo "   export LIGETRON_VERIFIER=/path/to/your/webgpu_verifier"
    echo "   export LIGETRON_SHADER_PATH=/path/to/your/shader"
    echo ""
    echo "🔍 Option 3: Add binaries to PATH"
    echo "   export PATH=\"/path/to/ligetron/binaries:\$PATH\""
    echo ""
    exit 1
fi

echo ""
echo "✅ Environment configured successfully!"
echo "   Prover: $LIGETRON_PROVER"
echo "   Verifier: $LIGETRON_VERIFIER"
echo "   Shader path: $LIGETRON_SHADER_PATH"
echo ""

# Test that binaries can be executed
echo "🧪 Testing binary execution..."

echo "Testing prover..."
if "$LIGETRON_PROVER" --help >/dev/null 2>&1 || [ $? -eq 1 ]; then
    echo "✅ Prover responds to --help"
else
    echo "⚠️  Prover --help returned unexpected exit code (might be normal)"
fi

echo "Testing verifier..."
if "$LIGETRON_VERIFIER" --help >/dev/null 2>&1 || [ $? -eq 1 ]; then
    echo "✅ Verifier responds to --help"
else
    echo "⚠️  Verifier --help returned unexpected exit code (might be normal)"
fi

echo ""
echo "🚀 Ready to run tests!"
echo ""

# Check if we should run tests automatically
if [ "$1" = "--run-tests" ]; then
    echo "Running Ligetron adapter tests..."
    cargo test --features native -- --nocapture
elif [ "$1" = "--run-integration" ]; then
    echo "Running integration tests only..."
    cargo test --features native integration_with_real_binaries -- --nocapture
else
    echo "To run tests, use one of:"
    echo "   cargo test --features native                    # All tests"
    echo "   cargo test --features native -- --nocapture    # All tests with output"
    echo "   ./setup_test_env.sh --run-tests               # Auto-run all tests"
    echo "   ./setup_test_env.sh --run-integration         # Auto-run integration tests"
fi
