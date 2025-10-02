#!/bin/bash

# Script to save diff between current changes and a base commit to a file
# Usage: ./show_diff.sh [output_file] ["path1,path2,path3,..."] [base_commit] [--no-tests]
#
# Examples:
#   ./show_diff.sh                                    # Save all changes to 'git_diff.txt' (vs HEAD)
#   ./show_diff.sh my_changes.txt                     # Save all changes to 'my_changes.txt' (vs HEAD)
#   ./show_diff.sh changes.txt "src,tests"            # Save only src/ and tests/ changes (vs HEAD)
#   ./show_diff.sh diff.txt "crates/neo-fold,neo-main/src"  # Save specific paths only (vs HEAD)
#   ./show_diff.sh changes.txt "" "main"              # Save all changes vs main branch
#   ./show_diff.sh changes.txt "src" "abc123"         # Save src/ changes vs commit abc123
#   ./show_diff.sh changes.txt "" "HEAD" --no-tests   # Save all changes excluding test files
#   ./show_diff.sh --no-tests                         # Save all changes excluding test files to 'git_diff.txt'

# Parse arguments and handle --no-tests flag
no_tests=false
args=()

# Process all arguments to separate --no-tests from positional args
for arg in "$@"; do
    if [ "$arg" = "--no-tests" ]; then
        no_tests=true
    elif [ "$arg" = "--help" ] || [ "$arg" = "-h" ]; then
        echo "Usage: $0 [output_file] [\"path1,path2,path3,...\"] [base_commit] [--no-tests]"
        echo ""
        echo "Examples:"
        echo "  $0                                    # Save all changes to 'git_diff.txt' (vs HEAD)"
        echo "  $0 my_changes.txt                     # Save all changes to 'my_changes.txt' (vs HEAD)"
        echo "  $0 changes.txt \"src,tests\"            # Save only src/ and tests/ changes (vs HEAD)"
        echo "  $0 diff.txt \"crates/neo-fold,neo-main/src\"  # Save specific paths only (vs HEAD)"
        echo "  $0 changes.txt \"\" \"main\"              # Save all changes vs main branch"
        echo "  $0 changes.txt \"src\" \"abc123\"         # Save src/ changes vs commit abc123"
        echo "  $0 changes.txt \"\" \"HEAD\" --no-tests   # Save all changes excluding test files"
        echo "  $0 --no-tests                         # Save all changes excluding test files to 'git_diff.txt'"
        echo ""
        echo "Options:"
        echo "  --no-tests    Exclude files in test directories (/tests/, /test/, *_test.*, test_*)"
        echo "  --help, -h    Show this help message"
        exit 0
    else
        args+=("$arg")
    fi
done

# Default values using processed arguments
output_file="${args[0]:-git_diff.txt}"
target_paths="${args[1]:-}"
base_commit="${args[2]:-HEAD}"

# Parse comma-separated paths into array
if [ -n "$target_paths" ]; then
    IFS=',' read -ra paths <<< "$target_paths"
    # Remove trailing slashes from paths
    for i in "${!paths[@]}"; do
        paths[$i]="${paths[$i]%/}"
    done
else
    paths=()
fi

# Function to filter out test files from a list of files
filter_test_files() {
    local files="$1"
    if [ "$no_tests" = true ]; then
        echo "$files" | grep -v '/tests/' | grep -v '/test/' | grep -v '_test\.' | grep -v 'test_'
    else
        echo "$files"
    fi
}

# Remove existing output file
rm -f "$output_file"
touch "$output_file"

# Validate base commit exists
if ! git rev-parse --verify "$base_commit" >/dev/null 2>&1; then
    echo "Error: Base commit '$base_commit' not found or invalid."
    echo "Please provide a valid commit hash, branch name, or tag."
    exit 1
fi

echo "Generating diff report..."
if [ "$base_commit" != "HEAD" ]; then
    echo "Using base commit: $base_commit"
fi
if [ ${#paths[@]} -gt 0 ]; then
    echo "Including only specified paths: ${paths[*]}"
fi
if [ "$no_tests" = true ]; then
    echo "Excluding test files (--no-tests flag active)"
fi

# Header
{
    echo "=============================================="
    echo "Git Diff Report - $(date)"
    echo "=============================================="
    echo "Branch: $(git branch --show-current)"
    echo "Commit: $(git rev-parse --short HEAD)"
    echo "Base commit: $(git rev-parse --short $base_commit)"
    echo ""
} >> "$output_file"

# Git Status Summary
{
    echo "=============================================="
    echo "Git Status Summary"
    echo "=============================================="
    if [ ${#paths[@]} -gt 0 ]; then
        git status --short -- "${paths[@]}"
    else
        git status --short
    fi
    echo ""
} >> "$output_file"

# Diff of Modified Files
{
    echo "=============================================="
    echo "Diff of Modified Files (against $base_commit)"
    echo "=============================================="
    if [ ${#paths[@]} -gt 0 ]; then
        if [ "$no_tests" = true ]; then
            # Get list of modified files, filter out test files, then get diff for remaining files
            modified_files=$(git diff $base_commit --name-only -- "${paths[@]}")
            filtered_files=$(filter_test_files "$modified_files")
            if [ -n "$filtered_files" ]; then
                echo "$filtered_files" | while IFS= read -r file; do
                    if [ -n "$file" ]; then
                        git diff $base_commit -- "$file"
                    fi
                done
            fi
        else
            git diff $base_commit -- "${paths[@]}"
        fi
    else
        if [ "$no_tests" = true ]; then
            # Get list of all modified files, filter out test files, then get diff for remaining files
            modified_files=$(git diff $base_commit --name-only)
            filtered_files=$(filter_test_files "$modified_files")
            if [ -n "$filtered_files" ]; then
                echo "$filtered_files" | while IFS= read -r file; do
                    if [ -n "$file" ]; then
                        git diff $base_commit -- "$file"
                    fi
                done
            fi
        else
            git diff $base_commit
        fi
    fi
    echo ""
} >> "$output_file"

# Untracked Files Content
{
    echo "=============================================="
    echo "Untracked Files Content"
    echo "=============================================="
    
    # Get untracked files, filtered by paths if specified
    if [ ${#paths[@]} -gt 0 ]; then
        untracked_files=""
        for path in "${paths[@]}"; do
            if [ -d "$path" ]; then
                path_untracked=$(git ls-files --others --exclude-standard "$path/")
            elif [ -f "$path" ]; then
                # Check if the specific file is untracked
                if git ls-files --error-unmatch "$path" >/dev/null 2>&1; then
                    path_untracked=""
                else
                    path_untracked="$path"
                fi
            else
                path_untracked=""
            fi
            if [ -n "$path_untracked" ]; then
                untracked_files="$untracked_files$path_untracked"$'\n'
            fi
        done
        untracked_files=$(echo "$untracked_files" | grep -v '^$')
    else
        untracked_files=$(git ls-files --others --exclude-standard)
    fi
    
    # Apply test file filtering to untracked files
    if [ "$no_tests" = true ] && [ -n "$untracked_files" ]; then
        untracked_files=$(filter_test_files "$untracked_files")
    fi
    
    if [ -n "$untracked_files" ]; then
        echo "Untracked files found:"
        echo "$untracked_files"
        echo ""
        
        echo "$untracked_files" | while IFS= read -r file; do
            if [ -f "$file" ]; then
                # Only show content for small files (less than 100KB)
                file_size=$(wc -c < "$file" 2>/dev/null || echo 0)
                if [ "$file_size" -lt 102400 ]; then
                    echo "--- Content of $file ---"
                    cat "$file"
                    echo ""
                    echo "--- End of $file ---"
                    echo ""
                else
                    echo "--- $file (too large to display, $(($file_size / 1024))KB) ---"
                    echo ""
                fi
            fi
        done
    else
        echo "No untracked files found."
    fi
    echo ""
} >> "$output_file"

# Summary
{
    echo "=============================================="
    echo "Summary"
    echo "=============================================="
    if [ ${#paths[@]} -gt 0 ]; then
        if [ "$no_tests" = true ]; then
            modified_files=$(git diff $base_commit --name-only -- "${paths[@]}")
            filtered_files=$(filter_test_files "$modified_files")
            modified_count=$(echo "$filtered_files" | grep -c '^' 2>/dev/null || echo 0)
        else
            modified_count=$(git diff $base_commit --name-only -- "${paths[@]}" | wc -l)
        fi
    else
        if [ "$no_tests" = true ]; then
            modified_files=$(git diff $base_commit --name-only)
            filtered_files=$(filter_test_files "$modified_files")
            modified_count=$(echo "$filtered_files" | grep -c '^' 2>/dev/null || echo 0)
        else
            modified_count=$(git diff $base_commit --name-only | wc -l)
        fi
    fi
    
    untracked_count=0
    if [ -n "$untracked_files" ]; then
        untracked_count=$(echo "$untracked_files" | wc -l)
    fi
    
    # Calculate file statistics for the summary
    if [ -f "$output_file" ]; then
        summary_size=$(wc -c < "$output_file")
        summary_lines=$(wc -l < "$output_file")
        summary_words=$(wc -w < "$output_file")
        # Approximate AI token count (1 token ≈ 4 characters for English text)
        summary_ai_tokens=$((summary_size / 4))
    else
        summary_size=0
        summary_lines=0
        summary_words=0
        summary_ai_tokens=0
    fi
    
    echo "Modified files: $modified_count"
    echo "Untracked files: $untracked_count"
    if [ ${#paths[@]} -gt 0 ]; then
        echo "Filtered paths: ${paths[*]}"
    fi
    if [ "$no_tests" = true ]; then
        echo "Test files excluded: --no-tests flag active"
    fi
    echo ""
    echo "File Statistics:"
    echo "Total size: ${summary_size} bytes"
    echo "Total lines: ${summary_lines}"
    echo "Total words: ${summary_words}"
    echo "Total AI tokens (est): ${summary_ai_tokens}"
} >> "$output_file"

# Show final statistics
if [ -f "$output_file" ]; then
    final_size=$(wc -c < "$output_file")
    final_lines=$(wc -l < "$output_file")
    final_words=$(wc -w < "$output_file")
    # Approximate AI token count (1 token ≈ 4 characters for English text)
    final_ai_tokens=$((final_size / 4))
    echo
    echo "=== Diff Report Generated ==="
    echo "Output file: $output_file"
    echo "Total size: ${final_size} bytes"
    echo "Total lines: ${final_lines}"
    echo "Total words: ${final_words}"
    echo "Total AI tokens (est): ${final_ai_tokens}"
    echo
    echo "View the diff with: cat $output_file"
    echo "Or open in editor: nano $output_file"
fi
