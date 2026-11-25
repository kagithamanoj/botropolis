//! `botropolis` command line interface.
//!
//! The full command surface lives in the library (`botropolis::cli`)
//! so tests can drive it; this binary just hands over argv and exits
//! with the returned code.

fn main() {
    std::process::exit(botropolis::cli::run(std::env::args().collect()));
}
