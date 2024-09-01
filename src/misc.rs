use std::fs::remove_dir_all;
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use validators::prelude::*;
use validators::url::Url;

#[derive(Validator)]
#[validator(http_url(local(Allow)))]
#[allow(dead_code)]
pub struct HttpURL {
    url: Url,
    is_https: bool,
}

pub fn cleanup(absolute_destination_path: PathBuf) {
    trace!(
        "Deleting the working directory {} ...",
        absolute_destination_path.to_str().unwrap()
    );
    remove_dir_all(absolute_destination_path).unwrap();
}

pub fn update() {
    stdout().flush().unwrap();
}

pub fn boot() {
    use crate::logger;
    logger::init();
    checkdep("yt-dlp");
    checkdep("ffmpeg");
    let _ = ctrlc::set_handler(move || {
        info!("Stopping ...");
        update();
        std::process::exit(0);
    });
}

fn checkdep(dep: &str) {
    trace!("Checking dependency {} ...", dep);
    let result_output = std::process::Command::new(dep).arg("--help").output();
    if let Err(e) = result_output {
        if let std::io::ErrorKind::NotFound = e.kind() {
            error!("{} is not found. Please install {} first.", dep, dep);
            std::process::exit(1);
        }
    }
}

pub fn sleep(secs: u32) {
    let time = Duration::from_secs(secs.into());
    thread::sleep(time);
}

pub fn url_is_valid(url: &str) -> bool {
    return HttpURL::parse_string(url).is_ok();
}
