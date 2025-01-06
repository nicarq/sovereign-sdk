## Sov-Benchmarks

This crate offers benchmark capabilities for the SDK.

## Submodules

### bench_files
This module contains utilities to generate benchmarks along with a folder containing the generated benchmarks. 

The [makefile](`./Makefile`) contains useful commands to locally generate benchmarks. One can simply run `make generate_and_run_benches` to generate and run benchmarks. The `SIZE` env variable can be used to specify the size of the benchmarks to be generated.

The README of the submodule [here](`./src/bench_files/README.md`) contains additional information regarding the benchmarks available.

### node
This module contains basic benchmarking commands to test node execution performances. This defines criterion benchmarks with simple transfers to be able to easily assert node's execution speed.