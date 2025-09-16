#!/bin/bash

# Ligetron Adapter Test Suite
# Comprehensive testing script for the Ligetron zkVM adapter in Sovereign SDK

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Test configuration
LIGETRON_DIR="crates/adapters/ligetron"
VERBOSE=false
INTEGRATION_ONLY=false
UNIT_ONLY=false
SHOW_HELP=false

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -i|--integration-only)
            INTEGRATION_ONLY=true
            shift
            ;;
        -u|--unit-only)
            UNIT_ONLY=true
            shift
            ;;
        -h|--help)
            SHOW_HELP=true
            shift
            ;;
        *)
            echo "Unknown option $1"
            SHOW_HELP=true
            shift
            ;;
    esac
done

# Help function
show_help() {
    echo -e "${CYAN}Ligetron Adapter Test Suite${NC}"
    echo ""
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -v, --verbose         Show detailed test output"
    echo "  -i, --integration-only Run only integration tests (requires binaries)"
    echo "  -u, --unit-only       Run only unit tests (no binaries required)"
    echo "  -h, --help           Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0                    # Run all tests"
    echo "  $0 -v                 # Run all tests with verbose output"
    echo "  $0 -u                 # Run only unit tests"
    echo "  $0 -i                 # Run only integration tests"
    echo ""
    echo "Environment Variables:"
    echo "  LIGETRON_PROVER       Path to webgpu_prover binary"
    echo "  LIGETRON_VERIFIER     Path to webgpu_verifier binary"
    echo "  LIGETRON_SHADER_PATH  Path to shader directory"
    echo ""
}

if [ "$SHOW_HELP" = true ]; then
    show_help
    exit 0
fi

# Header
echo -e "${CYAN}🔬 Ligetron Adapter Test Suite${NC}"
echo -e "${CYAN}================================${NC}"
echo ""

# Check if we're in the right directory
if [ ! -d "$LIGETRON_DIR" ]; then
    echo -e "${RED}❌ Error: Must run from Sovereign SDK root directory${NC}"
    echo -e "${RED}   Expected to find: $LIGETRON_DIR${NC}"
    exit 1
fi

# Change to Ligetron directory
cd "$LIGETRON_DIR"

# Check for binaries
check_binaries() {
    echo -e "${BLUE}🔍 Checking for Ligetron binaries...${NC}"
    
    local has_binaries=false
    
    # Check environment variables
    if [ -n "$LIGETRON_PROVER" ] && [ -n "$LIGETRON_VERIFIER" ]; then
        if [ -f "$LIGETRON_PROVER" ] && [ -f "$LIGETRON_VERIFIER" ]; then
            echo -e "${GREEN}✅ Found binaries via environment variables${NC}"
            echo -e "   Prover: $LIGETRON_PROVER"
            echo -e "   Verifier: $LIGETRON_VERIFIER"
            has_binaries=true
        fi
    fi
    
    # Check test_binaries directory
    if [ -f "./test_binaries/webgpu_prover" ] && [ -f "./test_binaries/webgpu_verifier" ]; then
        echo -e "${GREEN}✅ Found binaries in test_binaries/ directory${NC}"
        has_binaries=true
    fi
    
    # Check PATH
    if command -v webgpu_prover >/dev/null 2>&1 && command -v webgpu_verifier >/dev/null 2>&1; then
        echo -e "${GREEN}✅ Found binaries in PATH${NC}"
        has_binaries=true
    fi
    
    if [ "$has_binaries" = false ]; then
        echo -e "${YELLOW}⚠️  No Ligetron binaries found${NC}"
        echo -e "   Integration tests will FAIL if run"
        echo -e "   To run integration tests successfully:"
        echo -e "   1. Set LIGETRON_PROVER and LIGETRON_VERIFIER environment variables"
        echo -e "   2. Place binaries in ./test_binaries/"
        echo -e "   3. Add binaries to PATH"
    fi
    
    echo ""
    return $([ "$has_binaries" = true ])
}

# Run unit tests
run_unit_tests() {
    echo -e "${PURPLE}🧪 Running Unit Tests${NC}"
    echo -e "${PURPLE}=====================${NC}"
    
    local test_cmd="cargo test --features native --lib"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ Unit tests passed${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ Unit tests failed${NC}"
        return 1
    fi
}

# Run integration tests
run_integration_tests() {
    echo -e "${PURPLE}🔗 Running Integration Tests${NC}"
    echo -e "${PURPLE}============================${NC}"
    
    local test_cmd="cargo test --features native --test integration_with_real_binaries"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ Integration tests completed${NC}"
        echo -e "${YELLOW}   Note: Some tests may show warnings if binaries are missing${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ Integration tests failed${NC}"
        return 1
    fi
}

# Run SHA256 specific tests
run_sha256_tests() {
    echo -e "${PURPLE}🔍 Running SHA256 WASM Tests${NC}"
    echo -e "${PURPLE}============================${NC}"
    
    local test_cmd="cargo test --features native test_sha256"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ SHA256 tests completed${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ SHA256 tests failed${NC}"
        return 1
    fi
}

# Run mock tests
run_mock_tests() {
    echo -e "${PURPLE}🎭 Running Mock Tests${NC}"
    echo -e "${PURPLE}=====================${NC}"
    
    local test_cmd="cargo test --features native --test mock_ligetron"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ Mock tests passed${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ Mock tests failed${NC}"
        return 1
    fi
}

# Run native tests
run_native_tests() {
    echo -e "${PURPLE}🏠 Running Native Tests${NC}"
    echo -e "${PURPLE}========================${NC}"
    
    local test_cmd="cargo test --features native --test native"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ Native tests passed${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ Native tests failed${NC}"
        return 1
    fi
}

# Run configuration validation
run_config_validation() {
    echo -e "${PURPLE}⚙️  Running Configuration Validation${NC}"
    echo -e "${PURPLE}====================================${NC}"
    
    local test_cmd="cargo test --features native test_ligetron_config_validation"
    if [ "$VERBOSE" = true ]; then
        test_cmd="$test_cmd -- --nocapture"
    fi
    
    echo -e "${BLUE}Running: $test_cmd${NC}"
    echo ""
    
    if eval $test_cmd; then
        echo ""
        echo -e "${GREEN}✅ Configuration validation completed${NC}"
        return 0
    else
        echo ""
        echo -e "${RED}❌ Configuration validation failed${NC}"
        return 1
    fi
}

# Main execution
main() {
    local unit_result=0
    local integration_result=0
    local has_binaries=false
    
    # Check for binaries
    if check_binaries; then
        has_binaries=true
    fi
    
    # Run tests based on options
    if [ "$INTEGRATION_ONLY" = true ]; then
        if [ "$has_binaries" = false ]; then
            echo -e "${YELLOW}⚠️  Integration-only mode requested but no binaries found${NC}"
            echo -e "   Running configuration validation only${NC}"
            echo ""
            run_config_validation
            integration_result=$?
        else
            run_integration_tests
            integration_result=$?
            
            echo ""
            run_sha256_tests
            local sha256_result=$?
            
            echo ""
            run_config_validation
            local config_result=$?
            
            # Overall result for integration tests
            if [ $integration_result -eq 0 ] && [ $sha256_result -eq 0 ] && [ $config_result -eq 0 ]; then
                integration_result=0
            else
                integration_result=1
            fi
        fi
    elif [ "$UNIT_ONLY" = true ]; then
        run_unit_tests
        unit_result=$?
        
        echo ""
        run_mock_tests
        local mock_result=$?
        
        echo ""
        run_native_tests
        local native_result=$?
        
        # Overall result for unit tests
        if [ $unit_result -eq 0 ] && [ $mock_result -eq 0 ] && [ $native_result -eq 0 ]; then
            unit_result=0
        else
            unit_result=1
        fi
    else
        # Run all tests
        run_unit_tests
        unit_result=$?
        
        echo ""
        run_mock_tests
        local mock_result=$?
        
        echo ""
        run_native_tests
        local native_result=$?
        
        echo ""
        run_integration_tests
        integration_result=$?
        
        echo ""
        run_sha256_tests
        local sha256_result=$?
        
        echo ""
        run_config_validation
        local config_result=$?
        
        # Update results
        if [ $unit_result -eq 0 ] && [ $mock_result -eq 0 ] && [ $native_result -eq 0 ]; then
            unit_result=0
        else
            unit_result=1
        fi
        
        if [ $integration_result -eq 0 ] && [ $sha256_result -eq 0 ] && [ $config_result -eq 0 ]; then
            integration_result=0
        else
            integration_result=1
        fi
    fi
    
    # Summary
    echo ""
    echo -e "${CYAN}📊 Test Summary${NC}"
    echo -e "${CYAN}===============${NC}"
    
    if [ "$UNIT_ONLY" = false ]; then
        if [ $unit_result -eq 0 ]; then
            echo -e "${GREEN}✅ Unit Tests: PASSED${NC}"
        else
            echo -e "${RED}❌ Unit Tests: FAILED${NC}"
        fi
    fi
    
    if [ "$INTEGRATION_ONLY" = false ]; then
        if [ $integration_result -eq 0 ]; then
            echo -e "${GREEN}✅ Integration Tests: PASSED${NC}"
        else
            echo -e "${RED}❌ Integration Tests: FAILED${NC}"
        fi
    fi
    
    # Overall result
    local overall_result=0
    if [ "$UNIT_ONLY" = true ]; then
        overall_result=$unit_result
    elif [ "$INTEGRATION_ONLY" = true ]; then
        overall_result=$integration_result
    else
        if [ $unit_result -eq 0 ] && [ $integration_result -eq 0 ]; then
            overall_result=0
        else
            overall_result=1
        fi
    fi
    
    echo ""
    if [ $overall_result -eq 0 ]; then
        echo -e "${GREEN}🎉 All tests completed successfully!${NC}"
    else
        echo -e "${RED}💥 Some tests failed. Check the output above for details.${NC}"
    fi
    
    echo ""
    echo -e "${BLUE}💡 Tips:${NC}"
    echo -e "   • Use -v for verbose output"
    echo -e "   • Use -u to run only unit tests (no binaries needed)"
    echo -e "   • Use -i to run only integration tests (requires binaries)"
    echo -e "   • See TESTING.md for detailed setup instructions"
    
    return $overall_result
}

# Run main function
main "$@"
