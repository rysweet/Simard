//! `coin-gym` — the LOCAL COIN Gym harness CLI (Phase 4 of issue #2713).
//!
//! A thin entry point; all logic lives in `simard::coin_gym`. See
//! `docs/howto/run-the-coin-gym-harness.md`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    simard::dispatch_coin_gym_cli(std::env::args().skip(1))
}
