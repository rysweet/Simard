fn main() -> Result<(), Box<dyn std::error::Error>> {
    simard::dispatch_operator_probe(std::env::args().skip(1))
}
