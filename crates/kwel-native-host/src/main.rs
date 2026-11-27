use kwel_native_host::{
    install::{self, Layout},
    paths::RuntimePaths,
    server,
};
use kwel_protocol::{Code, Error, MAX_MESSAGE_BYTES, Message, Result};
use std::io;
use tokio::sync::mpsc;
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("kwel-native-host: {}", error.0.message());
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "linux")]
    if let Some(name) = std::env::args_os().next().and_then(|value| {
        std::path::Path::new(&value)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
    }) && matches!(
        name.as_str(),
        "preinst" | "prerm" | "kwel-connect.preinst" | "kwel-connect.prerm"
    ) {
        let hook = name.strip_prefix("kwel-connect.").unwrap_or(&name);
        return install::system_hook(hook, args.first().map(String::as_str).unwrap_or(""));
    }
    match args.first().map(String::as_str) {
        Some(origin) if origin.starts_with("chrome-extension://") => {
            if !kwel_protocol::valid_origin(origin, kwel_protocol::EXTENSION_ID) {
                return Err(Error(Code::PermissionRequired));
            }
            let _installation =
                install::session_guard(&Layout::current()?, &std::env::current_exe()?)?;
            // Rust's Windows stdio reads/writes pipe HANDLEs directly, without
            // the CRT text mode. Native messages are bytes on every platform.
            // Dedicated bounded threads also avoid Tokio's uncancellable stdin
            // blocking pool preventing Host termination on protocol failure.
            let (sender, input) = mpsc::channel(4);
            let (output, mut receiver) = mpsc::channel::<Message>(32);
            std::thread::spawn(move || {
                let mut stdin = io::stdin().lock();
                loop {
                    let message = kwel_protocol::read_frame(&mut stdin, MAX_MESSAGE_BYTES)
                        .and_then(|bytes| kwel_protocol::decode_native(&bytes));
                    if matches!(message, Err(Error(Code::Disconnected))) {
                        break;
                    }
                    let failed = message.is_err();
                    if sender.blocking_send(message).is_err() || failed {
                        break;
                    }
                }
            });
            std::thread::spawn(move || {
                let mut stdout = io::stdout().lock();
                while let Some(message) = receiver.blocking_recv() {
                    if kwel_protocol::write_frame(&mut stdout, &message, MAX_MESSAGE_BYTES).is_err()
                    {
                        break;
                    }
                }
            });
            server::run_transport(origin, RuntimePaths::current()?, input, output).await
        }
        Some("--version" | "version") if args.len() == 1 => {
            println!("{}", kwel_native_host::VERSION);
            Ok(())
        }
        Some("install") => {
            let source = if args.len() == 3 && args[1] == "--source" {
                std::path::PathBuf::from(&args[2])
            } else if args.len() == 1 {
                std::env::current_exe()?
                    .parent()
                    .ok_or(Error(Code::UnsafePath))?
                    .to_owned()
            } else {
                return Err(Error(Code::InvalidRequest));
            };
            install::install(
                &source,
                &Layout::current()?,
                &RuntimePaths::current()?,
                false,
            )?;
            Ok(())
        }
        Some("uninstall") if args.len() == 1 => {
            install::uninstall(&Layout::current()?, &RuntimePaths::current()?)
        }
        _ => Err(Error(Code::InvalidRequest)),
    }
}
