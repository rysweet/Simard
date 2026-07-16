//! `simard-kitchen` — the small "run a kitchen" app for the Gastronome identity.
//!
//! A thin entry point; all logic lives in `simard::gastronome`. It turns a
//! culinary/event brief into a costed, scheduled menu plan. See
//! `docs/howto/design-a-menu-with-gastronome.md`.

fn main() {
    if let Err(err) = simard::dispatch_gastronome_cli(std::env::args().skip(1)) {
        eprintln!("simard-kitchen: {err}");
        std::process::exit(1);
    }
}
