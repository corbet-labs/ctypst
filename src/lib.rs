#![forbid(unsafe_code)]
//! Safe, deterministic, embedded Typst for Rust applications.

mod engine;
mod error;
mod files;

#[cfg(feature = "document-fonts")]
pub mod fonts;
#[cfg(feature = "format")]
mod format;
#[cfg(feature = "pdf")]
mod pdf;
mod query;
#[cfg(feature = "raster")]
mod raster;

pub use engine::{
    CompileOutput, CompileRequest, DiagnosticsPolicy, Engine, EngineBuilder, Limits, PageConstraint,
};
pub use error::{Error, Result};
#[cfg(feature = "format")]
pub use format::format_source;
pub use query::query_json;
#[cfg(feature = "raster")]
pub use raster::RasterPage;
pub use typst_layout::PagedDocument as Document;
