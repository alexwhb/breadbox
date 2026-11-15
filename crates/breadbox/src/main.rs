//! Breadbox desktop app entry point.

fn main() -> eframe::Result<()> {
    breadbox::STARTED.get_or_init(std::time::Instant::now);
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("breadbox=info,warn"),
    )
    .init();
    // `breadbox --smoke <project> …`: start-up + run measurement (scripts/test-all.sh)
    let args: Vec<String> = std::env::args().collect();
    if let Some(smoke) = breadbox::smoke::SmokeArgs::parse(&args) {
        match smoke {
            Ok(a) => std::process::exit(breadbox::smoke::main(a)),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }
    breadbox::run()
}
