//! rstrace: offline photo -> real-scale SVG tracing for Shaper Trace style frames.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::unnecessary_map_or,
    clippy::manual_range_contains
)]
pub mod core;
pub mod load;

pub use crate::core::pipeline::{
    ExportFormat, FrameGeom, LineMode, Options, Pipeline, TraceResult, TRACE_CAPTURE_MM,
};
pub use crate::core::svg::SvgStyle;
