//! WoWTranslateCompanion.exe (Translate Companion) entry point.
#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

fn main() {
    let code = match tp_helper::cli::parse(std::env::args().skip(1)) {
        Ok(cli) => tp_helper::app::run(cli),
        Err(e) => {
            tp_helper::app::attach_console();
            eprintln!("{e}");
            eprint!("{}", tp_helper::cli::USAGE);
            64
        }
    };
    std::process::exit(code);
}
