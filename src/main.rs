#[macro_use]
extern crate log;
pub const CRATE_NAME: &str = module_path!();
pub const FILE_STORAGE: &str = "/tmp/telepirate-downloads";
mod bot;
mod database;
mod tracing;
mod misc;
mod pirate;

#[tokio::main]
async fn main() {
    misc::boot();
    bot::run().await;
}
