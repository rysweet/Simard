use simard::dispatch_operator_cli;

fn main() -> std::process::ExitCode {
    match dispatch_operator_cli(std::env::args().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
