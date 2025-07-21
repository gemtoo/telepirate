#[macro_use]
extern crate log;
pub const CRATE_NAME: &str = module_path!();
pub const FILE_STORAGE: &str = "/tmp/telepirate-downloads";
mod database;
mod engine;
mod misc;
mod pirate;
mod task;
mod tracing;
mod trackedmessage;

#[tokio::main]
async fn main() {
    misc::boot();
    engine::run().await;
}
