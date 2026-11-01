use std::{env, process::ExitCode};
fn main() -> ExitCode {
    match runtime_core::node::run_for_race(env::args().skip(1).collect(), runtime_core::node::RaceVariant::Cora) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("cora: {error}"); ExitCode::FAILURE }
    }
}
