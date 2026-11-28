#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_biz_ui::app;
use std::process::ExitCode;

fn main() -> ExitCode {
    app::main_with(
        teamcraft_app::default_db(),
        teamcraft_app::open,
        "team_demo_seed",
    )
}
