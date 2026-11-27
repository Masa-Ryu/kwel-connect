use kwel_client::{Client, Operation};
use kwel_protocol::{LocalResponse, Result, VERSION};
#[tokio::main(worker_threads = 2)]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let operation = kwel_client::parse_args(&args, "json");
    let version = match &operation {
        Ok(Operation::Request(request)) => request.version(),
        _ => VERSION,
    };
    let result = match operation {
        Ok(operation) => run(operation).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        let output = LocalResponse::error_at_version(version, "", error.0);
        if let Ok(json) = serde_json::to_string(&output) {
            println!("{json}");
        }
        std::process::exit(1);
    }
}
async fn run(operation: Operation) -> Result<()> {
    match operation {
        Operation::Version => println!("{}", kwel_native_host::VERSION),
        Operation::Profiles => println!(
            "{}",
            serde_json::to_string(
                &serde_json::json!({"v":1,"profiles":Client::current()?.profiles().await?})
            )?
        ),
        Operation::Request(request) => {
            let response = Client::current()?.request(request).await?;
            println!("{}", serde_json::to_string(&response)?);
        }
    }
    Ok(())
}
