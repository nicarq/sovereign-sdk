#!/bin/bash

set -eo pipefail

# Check for required tools
for tool in redocly yq; do
    command -v $tool >/dev/null 2>&1 || { echo >&2 "$tool is required but not installed. Aborting."; exit 1; }
done

rm -rf build
mkdir build

prepend_to_paths() {
    local input_file="$1"
    local output_file="$2"
    local prepend_value="$3"

    if ! cat "$input_file" | yq '.paths |= with_entries(.key |= "'"$prepend_value"'" + .)' > "$output_file"; then
        echo "Error: Failed to modify YAML. Please check if the input is valid YAML and yq is working correctly."
        return 1
    fi
}

prepend_to_paths "../sov-ledger-apis/openapi-v3.yaml" "./build/ledger.yaml" "/ledger"
prepend_to_paths "../sov-sequencer/openapi-v3.yaml" "./build/sequencer.yaml" "/sequencer"

output_spec="./build/openapi.yaml"

redocly join ./build/ledger.yaml ./build/sequencer.yaml --prefix-components-with-info-prop x-displayName -o "$output_spec"

yq -i '
  .info.title = "Sovereign Full Node API" |
  .info.description = "This is the REST API for the Sovereign SDK full node." |
  del(.info.x-displayName)
' "$output_spec"

echo "Full node API spec written to $output_spec"
