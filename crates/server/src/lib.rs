//! Local `REST` API service for `FormatWright` (roadmap G-33).
//!
//! Thin HTTP surface over the shared application-layer services in
//! `formatwright-core`: every conversion response carries the full
//! `ValidationReport` so callers always receive acceptance evidence.

#![forbid(unsafe_code)]

pub mod routes;
