//! WebAssembly boundary: the whole `ctypst` runtime for browsers and Node.
//!
//! This module only builds with the `wasm` feature. It exposes the engine,
//! the versioned measurement client, and the exporters over JSON values so
//! JavaScript callers cross exactly one boundary type. Fonts ship embedded;
//! callers configure nothing.

use std::collections::BTreeMap;

use wasm_bindgen::prelude::*;

use crate::measure::{MeasureClient, MeasureFormat, MeasureItem};
use crate::{CompileRequest, Engine};

fn error(message: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&message.to_string())
}

fn parse_map(raw: &str) -> Result<BTreeMap<String, String>, JsValue> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|fault| error(format!("inputs are not JSON: {fault}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| error("inputs must be a JSON object"))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|text| (key.clone(), text.to_owned()))
                .ok_or_else(|| error("input values must be strings"))
        })
        .collect()
}

/// One `ctypst` runtime: embedded fonts, measurement, compilation, export.
#[wasm_bindgen]
pub struct Ctypst {
    engine: Engine,
    measure: MeasureClient,
}

#[wasm_bindgen]
impl Ctypst {
    /// Open a runtime on the embedded document font pack.
    ///
    /// # Errors
    ///
    /// JavaScript sees a string error when no embedded font parses.
    #[wasm_bindgen(constructor)]
    pub fn open() -> Result<Ctypst, JsValue> {
        let engine = Engine::builder()
            .fonts(crate::fonts::documents())
            .build()
            .map_err(|fault| error(format!("cannot open ctypst: {fault}")))?;
        let measure = MeasureClient::new(crate::fonts::documents())
            .map_err(|fault| error(format!("cannot open measurement: {fault}")))?;
        Ok(Self { engine, measure })
    }

    /// Crate, engine, and protocol versions for cache keys and diagnostics.
    #[must_use]
    pub fn versions() -> String {
        serde_json::json!({
            "ctypst": env!("CARGO_PKG_VERSION"),
            "protocol": crate::measure::PROTOCOL_VERSION,
        })
        .to_string()
    }

    /// Measure a JSON array of items with a JSON format object.
    ///
    /// Returns `{results, calibration}` or a string error. Never throws.
    pub fn measure_all(&mut self, format_json: &str, items_json: &str) -> Result<String, JsValue> {
        let format: MeasureFormat = serde_json::from_str(format_json)
            .map_err(|fault| error(format!("format is not a measure format: {fault}")))?;
        let items: Vec<MeasureItem> = serde_json::from_str(items_json)
            .map_err(|fault| error(format!("items are not measure items: {fault}")))?;
        let results = self
            .measure
            .measure_all(&format, &items)
            .map_err(|fault| error(format!("measurement failed: {fault}")))?;
        let calibration = self.measure.calibration();
        serde_json::to_string(&serde_json::json!({
            "results": results,
            "calibration": calibration,
        }))
        .map_err(|fault| error(format!("cannot encode results: {fault}")))
    }

    /// Compile a source string and return the page count.
    pub fn page_count(&self, source: &str, inputs_json: &str) -> Result<usize, JsValue> {
        Ok(self.compile(source, inputs_json)?.pages().len())
    }

    /// Compile a source string and render one zero-based page to SVG text.
    pub fn render_page(
        &self,
        source: &str,
        inputs_json: &str,
        page: usize,
    ) -> Result<String, JsValue> {
        let document = self.compile(source, inputs_json)?;
        self.engine
            .svg_page(&document, page)
            .map_err(|fault| error(format!("SVG export failed: {fault}")))
    }

    /// Compile a source string and export a deterministic PDF (epoch 0).
    pub fn render_pdf(&self, source: &str, inputs_json: &str) -> Result<Vec<u8>, JsValue> {
        let document = self.compile(source, inputs_json)?;
        self.engine
            .pdf(&document, 0)
            .map_err(|fault| error(format!("PDF export failed: {fault}")))
    }

    fn compile(&self, source: &str, inputs_json: &str) -> Result<crate::Document, JsValue> {
        let inputs = parse_map(inputs_json)?;
        self.engine
            .compile(
                CompileRequest::new("main.typ".to_owned())
                    .source_file("main.typ", source.to_owned())
                    .inputs(inputs),
            )
            .map(|output| output.document)
            .map_err(|fault| error(format!("compilation failed: {fault}")))
    }
}
