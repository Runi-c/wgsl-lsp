use std::ops::ControlFlow;

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    MessageType, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
};
use ropey::Rope;

use crate::{
    document::{normalize_uri, OpenDocument},
    server::{NotifyResult, WgslServerState},
    validate::validate_document,
};

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocumentSyncOptions
pub fn text_document_sync_capability() -> TextDocumentSyncCapability {
    TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(TextDocumentSyncKind::INCREMENTAL),
        will_save: None,
        will_save_wait_until: None,
        save: None,
    }
    .into()
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didOpen
pub fn did_open_document(
    st: &mut WgslServerState,
    params: DidOpenTextDocumentParams,
) -> NotifyResult {
    let uri = normalize_uri(params.text_document.uri);
    st.open_documents.insert(
        uri.clone(),
        OpenDocument::ClientOwned(Rope::from_str(&params.text_document.text)),
    );
    st.log(MessageType::INFO, &format!("Opened document: {}", uri));
    if st.should_validate {
        validate_document(st, uri)
    } else {
        ControlFlow::Continue(())
    }
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didChange
pub fn did_change_document(
    st: &mut WgslServerState,
    params: DidChangeTextDocumentParams,
) -> NotifyResult {
    let uri = normalize_uri(params.text_document.uri);
    match st.ensure_document(&uri) {
        Ok(_) => (),
        Err(e) => return st.log(MessageType::ERROR, &e.message),
    }
    if let Some(doc) = st.open_documents.get_mut(&uri) {
        if let OpenDocument::ClientOwned(text) = doc {
            for change in params.content_changes {
                if let Some(range) = change.range {
                    let start = text.line_to_char(range.start.line as usize)
                        + range.start.character as usize;
                    let end =
                        text.line_to_char(range.end.line as usize) + range.end.character as usize;
                    text.remove(start..end);
                    text.insert(start, &change.text);
                } else {
                    *text = Rope::from_str(&change.text);
                }
            }
            validate_document(st, uri)
        } else {
            st.log(
                MessageType::ERROR,
                "Modified document is not owned by the client. This is a bug.",
            )
        }
    } else {
        st.log(
            MessageType::ERROR,
            "Modified document was not found in the open documents list. This is a bug.",
        )
    }
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didClose
pub fn did_close_document(
    st: &mut WgslServerState,
    params: DidCloseTextDocumentParams,
) -> NotifyResult {
    let uri = normalize_uri(params.text_document.uri);
    if st.open_documents.contains_key(&uri) {
        st.server_open(uri);
    } else {
        return st.log(
            MessageType::ERROR,
            "Closed document was not found in the open documents list. This is a bug.",
        );
    }
    ControlFlow::Continue(())
}
