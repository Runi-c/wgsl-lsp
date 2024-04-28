use std::future::{ready, Future};

use lsp_types::{
    notification::LogMessage,
    request::{Initialize, RegisterCapability, Shutdown},
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, InitializeParams,
    InitializeResult, InitializedParams, LogMessageParams, MessageType, Registration,
    RegistrationParams, Url,
};
use walkdir::WalkDir;

use crate::server::{get_server_info, NotifyResult, Result, WgslServerState};

use super::get_server_capabilities;

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#initialize
pub fn initialize(
    st: &mut WgslServerState,
    params: InitializeParams,
) -> impl Future<Output = Result<Initialize>> {
    // load .wgsl files from workspace folders
    let workspace_paths = params
        .workspace_folders
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.uri.to_file_path().ok())
        .filter_map(|p| p.into_os_string().into_string().ok());

    // load .wgsl files from additional include paths
    let include_paths = params
        .initialization_options
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|opts| opts.get("includePaths"))
        .and_then(|paths| paths.as_array())
        .cloned()
        .unwrap_or(vec![])
        .into_iter()
        .filter_map(|p| p.as_str().map(str::to_owned));

    for path in workspace_paths.chain(include_paths) {
        for path in WalkDir::new(&path)
            .into_iter()
            .filter_map(|f| f.ok())
            .map(|f| f.into_path())
            .filter(|p| p.extension().map(|ex| ex == "wgsl").unwrap_or(false) && p.is_file())
        {
            st.log(
                MessageType::INFO,
                &format!("Loading .wgsl file: {}", path.display()),
            );
            let uri = Url::from_file_path(path).unwrap();
            st.server_open(uri);
        }
    }

    ready(Ok(InitializeResult {
        server_info: Some(get_server_info()),
        capabilities: get_server_capabilities(),
    }))
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#initialized
pub fn initialized(st: &mut WgslServerState, _: InitializedParams) -> NotifyResult {
    let client = st.client.clone();

    tokio::spawn(async move {
        // this allows us to be notified about .wgsl files being created or deleted in the workspace
        // TODO: add handler for this notification
        match client
            .request::<RegisterCapability>(RegistrationParams {
                registrations: vec![Registration {
                    id: "workspace/didChangeWatchedFiles".to_string(),
                    method: "workspace/didChangeWatchedFiles".to_string(),
                    register_options: Some(
                        serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                            watchers: vec![FileSystemWatcher {
                                glob_pattern: "**/*.wgsl".to_string().into(),
                                kind: None,
                            }],
                        })
                        .unwrap(),
                    ),
                }],
            })
            .await
        {
            Ok(_) => {}
            Err(e) => {
                client
                    .notify::<LogMessage>(LogMessageParams {
                        typ: MessageType::ERROR,
                        message: format!(
                            "Failed to register workspace/didChangeWatchedFiles: {}",
                            e
                        ),
                    })
                    .unwrap();
            }
        };
    });

    st.log(MessageType::INFO, "server_initialized!")
}

/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#shutdown
pub fn shutdown(_: &mut WgslServerState, _: ()) -> impl Future<Output = Result<Shutdown>> {
    ready(Ok(()))
}
