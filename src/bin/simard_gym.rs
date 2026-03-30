fn main() -> Result<(), Box<dyn std::error::Error>> {
    simard::dispatch_legacy_gym_cli(std::env::args().skip(1))
}
