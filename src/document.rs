use std::{fs::File, io::Read};

use async_lsp::{ErrorCode, ResponseError};
use lsp_types::{MessageType, Url};
use ropey::Rope;

use crate::server::WgslServerState;

#[derive(Debug)]
pub enum OpenDocument {
    /// Client-owned documents expect to be edited, so they use a [Rope].
    ///
    /// It's unclear if this is helpful because the [Rope] will be written to a
    /// string whenever it needs to be validated anyway.
    ClientOwned(Rope),
    /// Server-owned documents are read-only and are stored as strings.
    ServerOwned(String),
}

impl OpenDocument {
    pub fn source(&self) -> String {
        let mut vec = Vec::new();
        match self {
            OpenDocument::ClientOwned(source) => {
                source.write_to(&mut vec).unwrap();
                String::from_utf8(vec).unwrap()
            }
            OpenDocument::ServerOwned(source) => source.clone(),
        }
    }
}

impl WgslServerState {
    /// Opens the document as server-owned if it's not already open. Does not preprocess.
    ///
    /// The document is guaranteed to exist in `open_documents` after a [Result::Ok].
    pub fn ensure_document(&mut self, uri: &Url) -> Result<(), ResponseError> {
        if self.open_documents.contains_key(uri) {
            return Ok(());
        }

        let mut text = String::new();
        if File::open(uri.as_str())
            .and_then(|mut file| file.read_to_string(&mut text))
            .is_ok()
        {
            self.open_documents
                .insert(uri.clone(), OpenDocument::ServerOwned(text));
            Ok(())
        } else {
            Err(ResponseError::new(
                ErrorCode::INVALID_PARAMS,
                "Requested document does not exist",
            ))
        }
    }

    /// Opens the document as server-owned and preprocesses it.
    pub fn server_open(&mut self, uri: Url) {
        let mut text = String::new();
        match File::open(uri.to_file_path().unwrap())
            .and_then(|mut file| file.read_to_string(&mut text))
        {
            Ok(_) => {
                self.open_documents
                    .insert(uri.clone(), OpenDocument::ServerOwned(text));
                self.preprocess(&uri);
                self.log(MessageType::INFO, &format!("Opened document: {}", uri));
            }
            Err(e) => {
                self.log(
                    MessageType::ERROR,
                    &format!("Failed to open document: {}", e),
                );
            }
        }
    }
}

/// Normalize file paths so that drive letter casing and colon encoding is consistent.
/// This is because we can't assume that the client will send the same casing/encoding as the server.
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#uri
pub fn normalize_uri(uri: Url) -> Url {
    if uri.scheme() != "file" {
        return uri;
    }
    match uri.to_file_path() {
        Ok(path) => Url::from_file_path(path).unwrap(),
        Err(_) => uri,
    }
}
