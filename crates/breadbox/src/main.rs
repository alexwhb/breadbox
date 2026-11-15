//! Breadbox desktop app entry point.

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("breadbox=info,warn")).init();
    breadbox::run()
}
