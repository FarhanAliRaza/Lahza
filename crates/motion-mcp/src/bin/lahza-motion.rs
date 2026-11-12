//! Stand-alone entry point for the motion engine (same as `lahza mcp` and
//! `lahza motion …` on the app binary), handy for agents and CI machines
//! without the desktop dependencies.
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_none_or(|a| a != "mcp" && a != "motion") {
        args.insert(0, "motion".into());
    }
    std::process::exit(lahza_motion_mcp::run_cli(&args).unwrap_or(2));
}
