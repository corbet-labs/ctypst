use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use typst::foundations::{Bytes, Dict, IntoValue};
use typst::text::Font;
use typst_as_lib::TypstEngine;

use crate::files::{SafeFsResolver, VirtualFiles};
use crate::{Document, Error, Result};

/// Finite resource limits applied to every engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum UTF-8 byte length of a virtual or requested path.
    pub max_path_bytes: usize,
    /// Maximum number of virtual or filesystem files in one namespace or compile.
    pub max_files: usize,
    /// Maximum bytes read from or supplied for one file.
    pub max_file_bytes: usize,
    /// Maximum aggregate bytes across virtual files or one filesystem compile.
    pub max_total_bytes: usize,
    /// Maximum number of supplied font files and parsed font faces.
    pub max_fonts: usize,
    /// Maximum bytes in one supplied font file.
    pub max_font_bytes: usize,
    /// Maximum aggregate bytes across supplied font files.
    pub max_total_font_bytes: usize,
    /// Maximum number of Typst system inputs in one compile request.
    pub max_inputs: usize,
    /// Maximum aggregate UTF-8 bytes across input keys and values.
    pub max_input_bytes: usize,
    /// Maximum accepted pages in a compiled document.
    pub max_pages: usize,
    /// Maximum bytes in one exported PDF.
    pub max_pdf_bytes: usize,
    /// Maximum pixels in one rasterized page.
    pub max_raster_pixels: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_path_bytes: 4 * 1024,
            max_files: 256,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_fonts: 64,
            max_font_bytes: 16 * 1024 * 1024,
            max_total_font_bytes: 64 * 1024 * 1024,
            max_inputs: 256,
            max_input_bytes: 2 * 1024 * 1024,
            max_pages: 32,
            max_pdf_bytes: 64 * 1024 * 1024,
            max_raster_pixels: 20_000_000,
        }
    }
}

/// How compilation warnings affect success.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticsPolicy {
    /// Treat every Typst warning as a failed build.
    #[default]
    DenyWarnings,
    /// Return warnings to the caller without failing.
    AllowWarnings,
}

/// Page-count requirement for one compilation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageConstraint {
    /// Require a non-empty document within the engine-wide maximum.
    #[default]
    Any,
    /// Require exactly this many pages.
    Exactly(usize),
    /// Require an inclusive page range.
    Between {
        /// Smallest accepted page count.
        minimum: usize,
        /// Largest accepted page count.
        maximum: usize,
    },
}

impl PageConstraint {
    fn accepts(self, actual: usize, hard_maximum: usize) -> bool {
        if actual == 0 || actual > hard_maximum {
            return false;
        }
        match self {
            Self::Any => true,
            Self::Exactly(expected) => actual == expected,
            Self::Between { minimum, maximum } => (minimum..=maximum).contains(&actual),
        }
    }

    fn describe(self, hard_maximum: usize) -> String {
        match self {
            Self::Any => format!("1-{hard_maximum}"),
            Self::Exactly(expected) => expected.to_string(),
            Self::Between { minimum, maximum } => format!("{minimum}-{maximum}"),
        }
    }
}

/// One compilation request.
#[derive(Clone, Debug)]
pub struct CompileRequest {
    source: String,
    inputs: BTreeMap<String, String>,
    sources: BTreeMap<String, String>,
    binaries: BTreeMap<String, Vec<u8>>,
    pages: PageConstraint,
    diagnostics: DiagnosticsPolicy,
}

impl CompileRequest {
    /// Create a request for a virtual or root-relative Typst source path.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            inputs: BTreeMap::new(),
            sources: BTreeMap::new(),
            binaries: BTreeMap::new(),
            pages: PageConstraint::Any,
            diagnostics: DiagnosticsPolicy::DenyWarnings,
        }
    }

    #[must_use]
    /// Add one Typst system input.
    pub fn input(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.insert(key.into(), value.into());
        self
    }

    #[must_use]
    /// Replace all Typst system inputs on this request.
    pub fn inputs(mut self, inputs: BTreeMap<String, String>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Overlay a virtual Typst source for this compilation only.
    #[must_use]
    pub fn source_file(mut self, path: impl Into<String>, source: impl Into<String>) -> Self {
        self.sources.insert(path.into(), source.into());
        self
    }

    /// Overlay a virtual binary asset for this compilation only.
    #[must_use]
    pub fn binary_file(mut self, path: impl Into<String>, binary: impl Into<Vec<u8>>) -> Self {
        self.binaries.insert(path.into(), binary.into());
        self
    }

    #[must_use]
    /// Apply a page-count contract to the compiled document.
    pub const fn pages(mut self, pages: PageConstraint) -> Self {
        self.pages = pages;
        self
    }

    #[must_use]
    /// Select how Typst warnings affect this compilation.
    pub const fn diagnostics(mut self, diagnostics: DiagnosticsPolicy) -> Self {
        self.diagnostics = diagnostics;
        self
    }
}

/// Successful Typst output and any explicitly accepted warnings.
pub struct CompileOutput {
    /// Compiled paged Typst document.
    pub document: Document,
    /// Warnings retained when the request explicitly allows them.
    pub warnings: Vec<String>,
}

/// Builder for a capability-limited embedded Typst engine.
pub struct EngineBuilder {
    root: Option<PathBuf>,
    fonts: Vec<Vec<u8>>,
    sources: Vec<(String, String)>,
    binaries: Vec<(String, Vec<u8>)>,
    limits: Limits,
    cache_age: usize,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self {
            root: None,
            fonts: Vec::new(),
            sources: Vec::new(),
            binaries: Vec::new(),
            limits: Limits::default(),
            cache_age: 2,
        }
    }
}

impl EngineBuilder {
    #[must_use]
    /// Permit root-relative reads below one canonical filesystem directory.
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    #[must_use]
    /// Add font bytes that will be validated and parsed at build time.
    pub fn fonts<I, B>(mut self, fonts: I) -> Self
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        self.fonts
            .extend(fonts.into_iter().map(|font| font.as_ref().to_vec()));
        self
    }

    /// Add a persistent virtual Typst source.
    pub fn source(mut self, path: impl Into<String>, source: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let source = source.into();
        let probe = VirtualFiles::new(self.limits);
        let _ = probe.apply_updates([(path.clone(), source.clone())], [])?;
        self.sources.push((path, source));
        Ok(self)
    }

    /// Add a persistent virtual binary asset.
    pub fn binary(mut self, path: impl Into<String>, binary: impl Into<Vec<u8>>) -> Result<Self> {
        let path = path.into();
        let binary = binary.into();
        let probe = VirtualFiles::new(self.limits);
        let _ = probe.apply_updates([], [(path.clone(), binary.clone())])?;
        self.binaries.push((path, binary));
        Ok(self)
    }

    #[must_use]
    /// Replace the finite resource limits.
    pub const fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    /// Set the number of compilation generations retained by Typst's memo cache.
    pub const fn cache_age(mut self, compilations: usize) -> Self {
        self.cache_age = compilations;
        self
    }

    /// Validate all resources and build the reusable engine.
    pub fn build(self) -> Result<Engine> {
        Self::check_limit(
            "font collection",
            "font file count",
            self.fonts.len(),
            self.limits.max_fonts,
        )?;
        let mut font_bytes = 0_usize;
        let mut parsed_fonts = Vec::new();
        for (index, font) in self.fonts.into_iter().enumerate() {
            Self::check_limit(
                &format!("font {index}"),
                "font file bytes",
                font.len(),
                self.limits.max_font_bytes,
            )?;
            font_bytes = font_bytes.saturating_add(font.len());
            Self::check_limit(
                "font collection",
                "total font bytes",
                font_bytes,
                self.limits.max_total_font_bytes,
            )?;
            let previous_faces = parsed_fonts.len();
            for parsed in Font::iter(Bytes::new(font)) {
                parsed_fonts.push(parsed);
                Self::check_limit(
                    "font collection",
                    "parsed font count",
                    parsed_fonts.len(),
                    self.limits.max_fonts,
                )?;
            }
            if parsed_fonts.len() == previous_faces {
                return Err(Error::InvalidFont { index });
            }
        }
        if parsed_fonts.is_empty() {
            return Err(Error::NoValidFont);
        }
        let virtual_files = VirtualFiles::new(self.limits);
        let _ = virtual_files.apply_updates(self.sources, self.binaries)?;
        let fs_resolver = self
            .root
            .as_deref()
            .map(|root| SafeFsResolver::new(root, self.limits))
            .transpose()?;
        let mut builder = TypstEngine::builder()
            .fonts(parsed_fonts)
            .add_file_resolver(virtual_files.clone());
        if let Some(resolver) = &fs_resolver {
            builder = builder.add_file_resolver(resolver.clone());
        }
        builder.comemo_evict_max_age(Some(self.cache_age));
        Ok(Engine {
            inner: builder.build(),
            virtual_files,
            fs_resolver,
            compile_lock: Mutex::new(()),
            limits: self.limits,
        })
    }

    fn check_limit(
        resource: &str,
        limit_name: &'static str,
        actual: usize,
        maximum: usize,
    ) -> Result<()> {
        if actual > maximum {
            return Err(Error::Limit {
                resource: resource.to_owned(),
                limit_name,
                actual,
                maximum,
            });
        }
        Ok(())
    }
}

/// A reusable embedded Typst compiler with no ambient network or font access.
pub struct Engine {
    inner: TypstEngine,
    virtual_files: VirtualFiles,
    fs_resolver: Option<SafeFsResolver>,
    compile_lock: Mutex<()>,
    limits: Limits,
}

impl Engine {
    #[must_use]
    /// Start building a capability-limited engine.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Compile one request under the engine lock and enforce its contracts.
    pub fn compile(&self, request: CompileRequest) -> Result<CompileOutput> {
        let _guard = self.compile_lock.lock().map_err(|_| Error::Poisoned)?;
        Self::check_size(
            &request.source,
            "path bytes",
            request.source.len(),
            self.limits.max_path_bytes,
        )?;
        Self::check_size(
            &request.source,
            "input count",
            request.inputs.len(),
            self.limits.max_inputs,
        )?;
        if let Some(resolver) = &self.fs_resolver {
            resolver.reset()?;
        }
        let input_bytes = request
            .inputs
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()))
            .fold(0_usize, usize::saturating_add);
        Self::check_size(
            &request.source,
            "input bytes",
            input_bytes,
            self.limits.max_input_bytes,
        )?;
        let snapshot = self
            .virtual_files
            .apply_updates(request.sources, request.binaries)?;
        let mut inputs = Dict::new();
        for (key, value) in &request.inputs {
            inputs.insert(key.as_str().into(), value.clone().into_value());
        }
        let result = (|| {
            let compiled = self
                .inner
                .compile_with_input::<_, _, Document>(request.source.as_str(), inputs);
            let warnings = compiled
                .warnings
                .iter()
                .map(|warning| format!("{warning:?}"))
                .collect::<Vec<_>>();
            if request.diagnostics == DiagnosticsPolicy::DenyWarnings && !warnings.is_empty() {
                return Err(Error::Warnings {
                    document: request.source.clone(),
                    diagnostics: warnings.join("; "),
                });
            }
            let document = compiled.output.map_err(|error| Error::Compile {
                document: request.source.clone(),
                diagnostics: error.to_string(),
            })?;
            let actual = document.pages().len();
            if !request.pages.accepts(actual, self.limits.max_pages) {
                return Err(Error::PageCount {
                    document: request.source.clone(),
                    actual,
                    expected: request.pages.describe(self.limits.max_pages),
                });
            }
            Ok(CompileOutput { document, warnings })
        })();
        if let Some(snapshot) = snapshot {
            self.virtual_files.restore(snapshot)?;
        }
        result
    }

    #[cfg(feature = "pdf")]
    pub(crate) fn check_pdf_size(&self, bytes: usize) -> Result<()> {
        Self::check_size("PDF", "PDF bytes", bytes, self.limits.max_pdf_bytes)
    }

    #[cfg(feature = "raster")]
    pub(crate) fn check_raster_size(&self, pixels: usize) -> Result<()> {
        Self::check_size(
            "raster page",
            "raster pixels",
            pixels,
            self.limits.max_raster_pixels,
        )
    }

    fn check_size(
        resource: &str,
        limit_name: &'static str,
        actual: usize,
        maximum: usize,
    ) -> Result<()> {
        if actual > maximum {
            return Err(Error::Limit {
                resource: resource.to_owned(),
                limit_name,
                actual,
                maximum,
            });
        }
        Ok(())
    }
}
