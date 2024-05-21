//! This crate provides utilities for building opinionated JSON APIs with Axum.
//!
//! # Design choices
//! - Response and request formats are *occasionally* inspired by
//!   [JSON:API](https://jsonapi.org/format/). This crate does *NOT* aim to be
//!   JSON:API compliant. More specifically, we completely disregard any parts
//!   of the spec that we find unnecessary or problematic for most use cases
//!   (e.g. "link objects" and "relationships", which only make sense when
//!   designing truly  HATEOAS-driven APIs).
//! - Query string parameters follow the bracket notation `foo[bar]` that was
//!   popularized by [`qs`](https://github.com/ljharb/qs).
//! - Pagination is cursor-based.
//!
//! # TODOs
//! - Add support for multi-column sorting: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/449>.

#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod axum_extractors;
mod pagination;
mod sorting;
pub mod test_utils;
pub mod types;
pub mod utils;

pub use axum_extractors::{PathWithErrorHandling, QueryStringValidation, ValidatedQuery};
pub use pagination::*;
pub use sorting::*;
