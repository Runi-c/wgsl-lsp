use std::{collections::HashMap, fmt::Debug, ops::ControlFlow};

use async_lsp::{router::Router, ClientSocket, ErrorCode, ResponseError};
use lsp_types::{
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Initialized, LogMessage,
        Notification,
    },
    request::{
        GotoDefinition, HoverRequest, Initialize, Request, SemanticTokensFullRequest, Shutdown,
    },
    LogMessageParams, MessageType, ServerInfo, Url,
};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga_oil::compose::Composer;

use crate::{
    document::OpenDocument,
    handlers::{
        document_sync::{did_change_document, did_close_document, did_open_document},
        lifecycle::{initialize, initialized, shutdown},
        semantic_tokens::semantic_tokens_full,
    },
    validate::CachedModule,
};

pub type Result<T> = async_lsp::Result<<T as Request>::Result, ResponseError>;
pub type NotifyResult = ControlFlow<async_lsp::Result<()>>;

pub fn get_server_info() -> ServerInfo {
    ServerInfo {
        name: "wgsl-lsp".to_string(),
        version: Some("0.1.".to_owned()),
    }
}

pub fn make_wgsl_router(client: ClientSocket) -> Router<WgslServerState> {
    let mut router = Router::new(WgslServerState::new(client));

    router
        // lifecycle
        .request::<Initialize, _>(initialize)
        .request::<Shutdown, _>(shutdown)
        .notification::<Initialized>(initialized)
        // document sync
        // TODO: .notification::<DidChangeConfiguration>(on_did_change_configuration)
        .notification::<DidOpenTextDocument>(did_open_document)
        .notification::<DidChangeTextDocument>(did_change_document)
        .notification::<DidCloseTextDocument>(did_close_document)
        // language features
        .request::<SemanticTokensFullRequest, _>(semantic_tokens_full)
        .request::<HoverRequest, _>(|_, _| async move { unimplemented!("Not yet implemented!") })
        .request::<GotoDefinition, _>(|_, _| async move { unimplemented!("Not yet implemented!") })
        .unhandled_notification(log_unhandled)
        .unhandled_event(log_unhandled)
        .unhandled_request(|st, req| {
            log_unhandled(st, req);
            async move {
                Err(ResponseError::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "Request not implemented",
                ))
            }
        });

    router
}

#[derive(Debug)]
pub struct WgslServerState {
    /// Handle to send messages to the language client. This can be cloned cheaply.
    pub client: ClientSocket,
    /// Open documents, either owned by the client or the server.
    pub open_documents: HashMap<Url, OpenDocument>,
    /// Mapping of module names/paths to their URLs.
    /// This will only contain either a path or a module name, not both.
    pub module_lookup: HashMap<String, Url>,
    /// Cache of successfully built modules.
    pub cached_modules: HashMap<Url, CachedModule>,
    /// Non-validating composer for building modules.
    pub composer: Composer,
    pub validator: Validator,
    /// Whether to validate newly opened/changed documents.
    ///
    /// This is false at first so that we get time to load all the documents and their dependencies.
    /// It should be set to true the first time the language server receives a request.
    pub should_validate: bool,
}

impl WgslServerState {
    pub fn new(client: ClientSocket) -> Self {
        Self {
            client,
            open_documents: HashMap::new(),
            module_lookup: HashMap::new(),
            cached_modules: HashMap::new(),
            composer: Composer::non_validating().with_capabilities(Capabilities::all()),
            validator: Validator::new(ValidationFlags::all(), Capabilities::all()),
            should_validate: false,
        }
    }

    /// Send a [LogMessage] notification to the client.
    ///
    /// This returns [ControlFlow::Break] if the message could not be sent due to the main loop stopping.
    pub fn log(&self, typ: MessageType, message: &str) -> NotifyResult {
        return self.notify::<LogMessage>(LogMessageParams {
            typ,
            message: message.to_string(),
        });
    }

    /// Send a [Notification] to the client.
    ///
    /// This returns [ControlFlow::Break] if the message could not be sent due to the main loop stopping.
    pub fn notify<T>(&self, params: T::Params) -> NotifyResult
    where
        T: Notification,
    {
        match self.client.notify::<T>(params) {
            Ok(_) => ControlFlow::Continue(()),
            Err(e) => ControlFlow::Break(Err(e)),
        }
    }
}

fn log_unhandled<T: Debug>(st: &mut WgslServerState, params: T) -> NotifyResult {
    st.log(MessageType::WARNING, &format!("Unhandled: {params:?}"))
}
