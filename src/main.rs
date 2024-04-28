use async_lsp::{
    client_monitor::ClientProcessMonitorLayer, concurrency::ConcurrencyLayer,
    panic::CatchUnwindLayer, server::LifecycleLayer, tracing::TracingLayer,
};
use server::make_wgsl_router;
use tower::ServiceBuilder;
use tracing::Level;

mod document;
mod handlers;
mod server;
mod validate;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        let router = make_wgsl_router(client.clone());

        ServiceBuilder::new()
            .layer(TracingLayer::default()) // Adds tracing spans to each request
            .layer(LifecycleLayer::default()) // Handles LSP server lifecycle
            .layer(CatchUnwindLayer::default()) // Catches panics and returns an error
            .layer(ConcurrencyLayer::default()) // Limits the number of concurrent requests
            .layer(ClientProcessMonitorLayer::new(client)) // Stops the server when the client process exits unexpectedly
            .service(router)
    });

    // Rest of this function is copied from https://github.com/oxalica/async-lsp/blob/main/examples/server_builder.rs

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();

    // Prefer truly asynchronous piped stdin/stdout without blocking tasks.
    #[cfg(unix)]
    let (stdin, stdout) = (
        async_lsp::stdio::PipeStdin::lock_tokio().unwrap(),
        async_lsp::stdio::PipeStdout::lock_tokio().unwrap(),
    );
    // Fallback to spawn blocking read/write otherwise.
    #[cfg(not(unix))]
    let (stdin, stdout) = (
        tokio_util::compat::TokioAsyncReadCompatExt::compat(tokio::io::stdin()),
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(tokio::io::stdout()),
    );

    server.run_buffered(stdin, stdout).await.unwrap();
}
