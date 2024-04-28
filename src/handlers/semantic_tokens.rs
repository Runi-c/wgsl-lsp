use std::{
    future::{ready, Future},
    ops::Range,
};

use async_lsp::{ErrorCode, ResponseError};
use bitflags::bitflags;
use lsp_types::{
    request::SemanticTokensFullRequest, MessageType, Position, SemanticToken,
    SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities,
};
use naga::{AddressSpace, Expression, Function, StorageAccess};

use crate::{
    document::normalize_uri,
    server::{Result, WgslServerState},
    validate::{calc_position, validate_document},
};

bitflags! {
    #[derive(Debug)]
    struct TokenModifiers: u32 {
        const READONLY = 1;
        const DEFAULT_LIBRARY = 2;
    }
}

#[derive(Debug)]
enum TokenType {
    Type,
    Struct,
    Function,
    Variable,
    Parameter,
    Number,
}

impl Into<u32> for &TokenType {
    fn into(self) -> u32 {
        match self {
            TokenType::Type => 0,
            TokenType::Struct => 1,
            TokenType::Function => 2,
            TokenType::Variable => 3,
            TokenType::Parameter => 4,
            TokenType::Number => 5,
        }
    }
}

impl From<&naga::Type> for TokenType {
    fn from(ty: &naga::Type) -> Self {
        match ty.inner {
            naga::TypeInner::Struct { .. } => TokenType::Struct,
            _ => TokenType::Type,
        }
    }
}

#[derive(Debug)]
struct Token {
    offset: usize,
    length: usize,
    ty: TokenType,
    modifiers: TokenModifiers,
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#semanticTokensOptions
pub fn semantic_tokens_capabilies() -> SemanticTokensServerCapabilities {
    SemanticTokensOptions {
        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
        range: Some(false),
        legend: SemanticTokensLegend {
            token_types: Vec::from([
                SemanticTokenType::TYPE,
                SemanticTokenType::STRUCT,
                SemanticTokenType::FUNCTION,
                SemanticTokenType::VARIABLE,
                SemanticTokenType::PARAMETER,
                SemanticTokenType::NUMBER,
            ]),
            token_modifiers: Vec::from([
                SemanticTokenModifier::READONLY,
                SemanticTokenModifier::DEFAULT_LIBRARY,
            ]),
        },
        ..Default::default()
    }
    .into()
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_semanticTokens
pub fn semantic_tokens_full(
    st: &mut WgslServerState,
    params: SemanticTokensParams,
) -> impl Future<Output = Result<SemanticTokensFullRequest>> {
    let uri = normalize_uri(params.text_document.uri);
    validate_document(st, uri.clone());

    let cached = match st.cached_modules.get(&uri) {
        Some(cached) => cached,
        None => {
            return ready(Err(ResponseError::new(
                ErrorCode::INVALID_PARAMS,
                "Requested document does not exist",
            )))
        }
    };

    let module = &cached.module;
    let source = &st
        .composer
        .module_sets
        .get(&cached.module_name)
        .unwrap()
        .sanitized_source;
    let mut tokens = Vec::new();

    // Collect all the tokens we can.
    for (handle, constant) in module.constants.iter() {
        if let Some(range) = module.constants.get_span(handle).to_range() {
            let src = &source[range.start..range.end];
            if let Some(name) = &constant.name {
                let start = range.start + src.find(name).unwrap();
                tokens.push(Token {
                    offset: start,
                    length: name.len(),
                    ty: TokenType::Variable,
                    modifiers: TokenModifiers::READONLY,
                });
            }
        }
    }

    for (handle, ty) in module.types.iter() {
        if let Some(range) = module.types.get_span(handle).to_range() {
            tokens.push(Token {
                offset: range.start,
                length: range.end - range.start,
                ty: ty.into(),
                modifiers: TokenModifiers::READONLY,
            })
        }
    }

    for (handle, var) in module.global_variables.iter() {
        if let Some(range) = module.global_variables.get_span(handle).to_range() {
            let src = &source[range.start..range.end];
            if let Some(name) = &var.name {
                let start = range.start + src.find(name).unwrap();
                tokens.push(Token {
                    offset: start,
                    length: name.len(),
                    ty: TokenType::Variable,
                    modifiers: TokenModifiers::empty(),
                });
            }
        }
    }

    let get_expression_token = |range: Range<usize>,
                                expr: &Expression,
                                fun: Option<&Function>|
     -> Option<Token> {
        let offset = range.start;
        let length = range.end - range.start;
        match expr {
            Expression::Constant(_) => Some(Token {
                offset,
                length,
                ty: TokenType::Variable,
                modifiers: TokenModifiers::READONLY,
            }),
            Expression::FunctionArgument(_) => Some(Token {
                offset,
                length,
                ty: TokenType::Parameter,
                modifiers: TokenModifiers::empty(),
            }),
            Expression::GlobalVariable(global) => {
                let var = module.global_variables.try_get(*global).unwrap();
                let modifiers = match var.space {
                    AddressSpace::Handle | AddressSpace::PushConstant | AddressSpace::Uniform => {
                        TokenModifiers::READONLY
                    }
                    AddressSpace::Storage { access } if !access.contains(StorageAccess::LOAD) => {
                        TokenModifiers::READONLY
                    }
                    _ => TokenModifiers::empty(),
                };
                Some(Token {
                    offset,
                    length,
                    ty: TokenType::Variable,
                    modifiers,
                })
            }
            Expression::Literal(_) => Some(Token {
                offset,
                length,
                ty: TokenType::Number,
                modifiers: TokenModifiers::empty(),
            }),
            Expression::LocalVariable(var) => {
                let span = fun.unwrap().local_variables.get_span(*var);
                st.log(
                    MessageType::LOG,
                    format!("Local variable found: {range:?} {span:?}").as_str(),
                );
                Some(Token {
                    offset,
                    length,
                    ty: TokenType::Variable,
                    modifiers: TokenModifiers::empty(),
                })
            }
            Expression::CallResult(fun) => {
                let fun = module.functions.try_get(*fun).unwrap();
                let name = fun.name.as_ref().unwrap();
                let src = &source[range.start..range.end];
                let start = range.start + src.find(name).unwrap();
                Some(Token {
                    offset: start,
                    length: name.len(),
                    ty: TokenType::Function,
                    modifiers: TokenModifiers::empty(),
                })
            }
            _ => None,
        }
    };

    for (handle, expr) in module.const_expressions.iter() {
        if let Some(range) = module.const_expressions.get_span(handle).to_range() {
            if let Some(token) = get_expression_token(range, expr, None) {
                tokens.push(token);
            }
        }
    }

    for (handle, fun) in module.functions.iter() {
        if let Some(range) = module.functions.get_span(handle).to_range() {
            if let Some(name) = &fun.name {
                let src = &source[range.start..range.end];
                let start = range.start + src.find(name).unwrap();
                tokens.push(Token {
                    offset: start,
                    length: name.len(),
                    ty: TokenType::Function,
                    modifiers: TokenModifiers::empty(),
                });
            }
        }
        for (handle, expr) in fun.expressions.iter() {
            if let Some(range) = fun.expressions.get_span(handle).to_range() {
                st.log(
                    MessageType::LOG,
                    format!("Expression found: {range:?} {expr:?}").as_str(),
                );
                if let Some(token) = get_expression_token(range, expr, Some(fun)) {
                    tokens.push(token);
                }
            }
        }
    }

    // Calculate the relative positions of the tokens
    tokens.sort_by_key(|token| token.offset);

    let mut semantic_tokens = Vec::new();
    let mut last_pos = Position::new(0, 0);
    for token in &tokens {
        let pos = calc_position(&source, token.offset);
        semantic_tokens.push(SemanticToken {
            delta_line: pos.line - last_pos.line,
            delta_start: if pos.line == last_pos.line {
                pos.character - last_pos.character
            } else {
                pos.character
            },
            length: token.length as u32,
            token_type: (&token.ty).into(),
            token_modifiers_bitset: token.modifiers.bits(),
        });
        last_pos = pos;
    }

    ready(Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: semantic_tokens,
    }))))
}
