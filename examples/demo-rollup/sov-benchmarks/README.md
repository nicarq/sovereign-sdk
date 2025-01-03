## Sov-Benchmarks

This crate offers benchmark capabilities for the SDK.

## Submodules

### bench_files
This module contains utilities to generate benchmarks along with a folder containing the generated benchmarks. 

The makefile contains useful commands to locally generate benchmarks.

The README of the submodule [here](`./src/bench_files/README.md`) contains additional information regarding the benchmarks available.

### node
This module contains basic benchmarking commands to test node execution performances. This defines criterion benchmarks with simple transfers to be able to easily assert node's execution speed.