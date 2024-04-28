use lsp_types::ServerCapabilities;

use self::{
    document_sync::text_document_sync_capability, semantic_tokens::semantic_tokens_capabilies,
};

pub mod document_sync;
pub mod lifecycle;
pub mod semantic_tokens;

pub fn get_server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(text_document_sync_capability()),
        semantic_tokens_provider: Some(semantic_tokens_capabilies()),
        ..Default::default()
    }
}
