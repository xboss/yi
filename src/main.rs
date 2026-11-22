#[cfg(target_os = "macos")]
mod api;
#[cfg(target_os = "macos")]
mod macos_runloop;
#[cfg(target_os = "macos")]
mod translator;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This server requires macOS 26 or later and installed Translation language packs.");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{net::SocketAddr, sync::mpsc, time::Duration};

    // Bind to localhost by default: this HTTP API intentionally has no authentication.
    let bind: SocketAddr = std::env::var("TRANSLATE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
        .parse()?;

    let listener = std::net::TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;

    let translator = translator::TranslatorHandle::start()?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);

    let server_thread = std::thread::Builder::new()
        .name("axum-server".into())
        .spawn(move || api::run_http_server(listener, translator, ready_tx))?;

    // Wait for the Tokio runtime and the Axum service to be initialized.
    // Do not enter CFRunLoop if startup failed.
    match ready_rx.recv_timeout(Duration::from_secs(15))? {
        Ok(()) => {}
        Err(message) => return Err(message.into()),
    }

    println!("Apple Translation HTTP server: http://{address}");
    println!("POST /translate  |  GET /healthz");
    println!("Press Ctrl-C to stop (language packs must already be installed).");

    // DO NOT use #[tokio::main] here: Translation.framework completes work on
    // the main dispatch queue. The macOS main thread must keep servicing it.
    macos_runloop::run_main_loop();

    server_thread.join().map_err(|_| "Axum thread panicked")?;
    Ok(())
}
