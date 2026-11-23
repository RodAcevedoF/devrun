mod adapters;
mod app;
mod cli;
mod slices;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run()
}
