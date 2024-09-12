## sov-api-spec

This package contains configuration and utility functionality for the full node API specs.

#### `build_spec.sh`

This script merges relevant sub openapi specs to create a single openapi spec to be consumed by stainless.

Required tools: `yq`, `redocly`.

It will:

- Prepend url paths to specs so they contain correct paths
- Merge the specs using `redocly`
- Update any individual spec specific information to be full node specific

The built spec will be outputted to `./build/openapi.yaml`.
