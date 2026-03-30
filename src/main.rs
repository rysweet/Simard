use simard::dispatch_operator_cli;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(std::env::args().skip(1))
}
