use crate::CRATE_NAME;
use fern;
use fern::colors::{Color, ColoredLevelConfig};
use humantime::format_rfc3339_seconds as timestamp;
use std::time::SystemTime;

pub fn init() {
    let colors = ColoredLevelConfig::new()
        .info(Color::Green)
        .debug(Color::Magenta)
        .trace(Color::Blue)
        .warn(Color::Yellow)
        .error(Color::Red);

    fern::Dispatch::new()
        .format(move |out, message, record| {
            let mut level = colors.color(record.level()).to_string();
            let time = timestamp(SystemTime::now())
                .to_string()
                .replace("T", " ")
                .replace("Z", "");
            if level.len() == 13 {
                level += " ";
            }
            out.finish(format_args!("[ {} {} ] {}", time, level, message))
        })
        .level(log::LevelFilter::Off)
        .level_for(CRATE_NAME.replace("-", "_"), log::LevelFilter::Trace)
        .chain(std::io::stdout())
        .apply()
        .unwrap();
    info!("Started up.");
}
