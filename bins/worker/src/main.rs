//! AIT Run worker entry point.

fn main() {
    std::panic::set_hook(Box::new(|_| eprintln!("ait-worker: execution panic")));
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["--stdio", "--protocol-major", "1"] {
        eprintln!("ait-worker: unsupported invocation or protocol major");
        std::process::exit(2);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        eprintln!("ait-worker: runtime unavailable");
        std::process::exit(2);
    };
    let result = runtime.block_on(ait_worker::stdio::serve());
    if let Err(code) = &result {
        eprintln!("ait-worker: {code}");
    }
    // stdio's blocking OS reader may outlive an aborted Tokio reader. Execution
    // and tools are drained before this bounded runtime shutdown.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    ait_sandbox::cleanup_worker_group();
    std::process::exit(i32::from(result.is_err()));
}
