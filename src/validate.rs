use std::str::FromStr;

use lsp_types::{
    notification::PublishDiagnostics, Diagnostic, DiagnosticRelatedInformation, Location, Position,
    PublishDiagnosticsParams, Range, Url,
};
use naga::Module;
use naga_oil::compose::{
    get_preprocessor_data, ComposableModuleDescriptor, Composer, ComposerError, ComposerErrorInner,
    NagaModuleDescriptor,
};

use crate::server::{NotifyResult, WgslServerState};

#[derive(Debug)]
pub struct CachedModule {
    pub module: Module,
    /// This will either be a plain name or a filepath
    /// depending on if the module contains #define_import_path.
    pub module_name: String,
    /// Module names of dependencies.
    pub dependencies: Vec<String>,
}

impl WgslServerState {
    /// Preprocess a document and add it to module lookup.
    ///
    /// Returns the cloned source, module name, and dependencies.
    pub fn preprocess(&mut self, uri: &Url) -> (String, String, Vec<String>) {
        let document = self.open_documents.get(uri).unwrap();
        let source = document.source();

        // from bevy_render Shader::preprocess
        let (module_name, imports, _) = get_preprocessor_data(&source);
        let module_name = module_name.unwrap_or_else(|| uri.as_str().to_owned());
        let dependencies = imports
            .into_iter()
            .map(|def| {
                if def.import.starts_with('\"') {
                    def.import
                        .chars()
                        .skip(1)
                        .take_while(|c| *c != '\"')
                        .collect()
                } else {
                    def.import
                }
            })
            .collect();

        self.module_lookup.insert(module_name.clone(), uri.clone());

        (source, module_name, dependencies)
    }

    /// Add a module to the composer and validate it.
    ///
    /// This will also walk the dependencies and make sure they're added first, as required by the composer.
    pub fn add_module(&mut self, uri: &Url) -> Result<(), ValidationError> {
        let (source, module_name, dependencies) = self.preprocess(uri);
        dependencies
            .iter()
            .map(|dep| {
                if let Some(uri) = self.module_lookup.get(dep).cloned() {
                    self.add_module(&uri)
                } else {
                    Err(import_error(uri.clone(), &source, dep))
                }
            })
            .find(|r| r.is_err())
            .unwrap_or(Ok(()))?; // propagate the first error

        match self
            .composer
            .add_composable_module(ComposableModuleDescriptor {
                as_name: Some(module_name.clone()),
                file_path: uri.as_str(),
                source: &source,
                ..Default::default()
            }) {
            Ok(_) => {
                self.notify::<PublishDiagnostics>(PublishDiagnosticsParams {
                    uri: uri.clone(),
                    diagnostics: Vec::new(),
                    version: None,
                });
            }
            Err(err) => {
                if Url::from_str(err.source.path(&self.composer)).unwrap() != *uri {
                    self.notify::<PublishDiagnostics>(PublishDiagnosticsParams {
                        uri: uri.clone(),
                        diagnostics: vec![Diagnostic {
                            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                            message: format!("Error in module: {module_name}"),
                            ..Default::default()
                        }],
                        version: None,
                    });
                }
                self.notify::<PublishDiagnostics>(composer_error_to_diagnostic(
                    err,
                    &self.composer,
                ));
            }
        };

        Ok(())
    }
}

#[derive(Debug)]
pub enum ValidationError {
    ComposerError(ComposerError),
    ImportNotFound(Url, Range, String),
}

impl From<ComposerError> for ValidationError {
    fn from(err: ComposerError) -> Self {
        ValidationError::ComposerError(err)
    }
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_publishDiagnostics
/// TODO: https://github.com/gfx-rs/wgpu/issues/5295
pub fn validate_document(st: &mut WgslServerState, uri: Url) -> NotifyResult {
    st.should_validate = true;
    let diagnostics = match validate_document_inner(st, uri.clone()) {
        Ok(_) => PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics: Vec::new(),
            version: None,
        },
        Err(err) => match err {
            ValidationError::ComposerError(err) => composer_error_to_diagnostic(err, &st.composer),
            ValidationError::ImportNotFound(uri, range, name) => PublishDiagnosticsParams {
                uri,
                diagnostics: vec![Diagnostic {
                    range,
                    message: format!("Import not found: {}", name),
                    ..Default::default()
                }],
                version: None,
            },
        },
    };

    if diagnostics.uri != uri {
        let module_name = st
            .module_lookup
            .iter()
            .find(|(_, &ref u)| *u == diagnostics.uri)
            .unwrap()
            .0;
        let document = st.open_documents.get(&uri).unwrap();
        let source = document.source();
        let start = source.find(module_name).unwrap_or(0);
        st.notify::<PublishDiagnostics>(PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics: vec![Diagnostic {
                range: calc_range(&source, start, start + module_name.len()),
                message: format!("Error in module: {module_name}"),
                ..Default::default()
            }],
            version: None,
        });
    }

    st.notify::<PublishDiagnostics>(diagnostics)
}

fn validate_document_inner(st: &mut WgslServerState, uri: Url) -> Result<(), ValidationError> {
    let old_module_name = st.cached_modules.get(&uri).map(|m| m.module_name.clone());
    if let Some(old_module_name) = &old_module_name {
        st.module_lookup.remove(old_module_name);
        // this will remove all dependents as well
        st.composer.remove_composable_module(old_module_name);
    }

    let (source, module_name, dependencies) = st.preprocess(&uri);
    let source = source.as_str();
    for dep in &dependencies {
        if let Some(uri) = st.module_lookup.get(dep).cloned() {
            st.add_module(&uri)?;
        } else {
            return Err(import_error(uri, source, dep));
        }
    }

    st.composer
        .add_composable_module(ComposableModuleDescriptor {
            as_name: Some(module_name.clone()),
            file_path: uri.as_str(),
            source,
            ..Default::default()
        })?;

    let module = st.composer.make_naga_module(NagaModuleDescriptor {
        source,
        file_path: uri.as_str(),
        ..Default::default()
    })?;

    st.composer.validate = true;
    // Use composer since it's the only one that knows the correct span positions to map the error
    let validator_result = st.composer.make_naga_module(NagaModuleDescriptor {
        source,
        file_path: uri.as_str(),
        ..Default::default()
    }); // Don't return early here so that we can still cache the possibly invalid module
    st.composer.validate = false;

    st.cached_modules.insert(
        uri.clone(),
        CachedModule {
            module,
            module_name,
            dependencies,
        },
    );

    validator_result?;

    Ok(())
}

pub fn calc_position(source: &str, position: usize) -> Position {
    let prefix = &source[..position];
    let line_number = prefix.matches('\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_position = source[line_start..position].chars().count() as u32;

    Position::new(line_number, line_position)
}

fn calc_range(source: &str, start: usize, end: usize) -> Range {
    Range::new(calc_position(source, start), calc_position(source, end))
}

fn import_error(uri: Url, source: &str, name: &str) -> ValidationError {
    let start = source.find(name).unwrap_or(0);
    ValidationError::ImportNotFound(
        uri,
        calc_range(source, start, start + name.len()),
        name.to_string(),
    )
}

fn composer_error_to_diagnostic(
    err: ComposerError,
    composer: &Composer,
) -> PublishDiagnosticsParams {
    let source = err.source.source(composer);
    let source_offset = err.source.offset();

    // https://github.com/bevyengine/naga_oil/issues/76
    // 21 is the SPAN_SHIFT
    let map_span = |rng: core::ops::Range<usize>| -> core::ops::Range<usize> {
        ((rng.start & ((1 << 21) - 1)).saturating_sub(source_offset))
            ..((rng.end & ((1 << 21) - 1)).saturating_sub(source_offset))
    };

    let uri = Url::from_str(err.source.path(composer)).unwrap();
    let message = err.inner.to_string();

    let empty_diagnostic = || -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            message: message.clone(),
            ..Default::default()
        }
    };

    let simple_diagnostic = |range: core::ops::Range<usize>| -> Diagnostic {
        Diagnostic {
            range: calc_range(&source, range.start, range.end),
            message: message.clone(),
            ..Default::default()
        }
    };

    let diagnostic_with_labels = |labels: Vec<(core::ops::Range<usize>, String)>| -> Diagnostic {
        let widest_label = labels
            .iter()
            .max_by(|a, b| a.0.len().cmp(&b.0.len()))
            .unwrap();
        let contained_label = labels.iter().find(|(rng, _)| {
            !rng.eq(&widest_label.0)
                && widest_label.0.start <= rng.start
                && widest_label.0.end >= rng.end
        });
        let (primary_rng, _) = contained_label.unwrap_or(widest_label);
        Diagnostic {
            range: calc_range(&source, primary_rng.start, primary_rng.end),
            message: message.clone(),
            related_information: Some(
                labels
                    .into_iter()
                    .map(|(rng, extra)| DiagnosticRelatedInformation {
                        location: Location::new(
                            uri.clone(),
                            calc_range(&source, rng.start, rng.end),
                        ),
                        message: extra,
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    };

    // Adapted from https://github.com/bevyengine/naga_oil/blob/33e57e488660aaeee81fa928454e51c215f9d0be/src/compose/error.rs#L190
    // See also: https://github.com/bevyengine/naga_oil/issues/76
    let diagnostic = match &err.inner {
        ComposerErrorInner::DecorationInSource(range) => simple_diagnostic(range.clone()),
        ComposerErrorInner::InvalidIdentifier { at, .. } => {
            simple_diagnostic(map_span(at.to_range().unwrap_or(0..0)))
        }

        ComposerErrorInner::ImportNotFound(_, pos)
        | ComposerErrorInner::ImportParseError(_, pos)
        | ComposerErrorInner::NotEnoughEndIfs(pos)
        | ComposerErrorInner::TooManyEndIfs(pos)
        | ComposerErrorInner::ElseWithoutCondition(pos)
        | ComposerErrorInner::UnknownShaderDef { pos, .. }
        | ComposerErrorInner::UnknownShaderDefOperator { pos, .. }
        | ComposerErrorInner::InvalidShaderDefComparisonValue { pos, .. }
        | ComposerErrorInner::OverrideNotVirtual { pos, .. }
        | ComposerErrorInner::GlslInvalidVersion(pos)
        | ComposerErrorInner::DefineInModule(pos)
        | ComposerErrorInner::InvalidShaderDefDefinitionValue { pos, .. } => {
            simple_diagnostic(*pos..*pos)
        }

        ComposerErrorInner::WgslBackError(..)
        | ComposerErrorInner::GlslBackError(..)
        | ComposerErrorInner::InconsistentShaderDefValue { .. }
        | ComposerErrorInner::RedirectError(..)
        | ComposerErrorInner::NoModuleName => empty_diagnostic(),

        ComposerErrorInner::HeaderValidationError(v)
        | ComposerErrorInner::ShaderValidationError(v) => diagnostic_with_labels(
            v.spans()
                .map(|(span, desc)| (map_span(span.to_range().unwrap_or(0..0)), desc.to_string()))
                .collect(),
        ),
        ComposerErrorInner::WgslParseError(e) => diagnostic_with_labels(
            e.labels()
                .map(|(range, msg)| (map_span(range.to_range().unwrap()), msg.to_string()))
                .collect(),
        ),
        ComposerErrorInner::GlslParseError(e) => diagnostic_with_labels(
            e.iter()
                .map(|naga::front::glsl::Error { kind, meta }| {
                    (map_span(meta.to_range().unwrap_or(0..0)), kind.to_string())
                })
                .collect(),
        ),
    };

    PublishDiagnosticsParams {
        uri,
        diagnostics: vec![diagnostic],
        version: None,
    }
}
