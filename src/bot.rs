use crate::misc::url_is_valid;
use crate::{
    database::{self, forget_deleted_messages},
    misc::{cleanup, sleep},
    pirate::{self, FileType},
};
use dptree::case;
use std::error::Error;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use teloxide::types::ChatKind;
use teloxide::types::InputFile;
use teloxide::{
    dispatching::UpdateHandler, prelude::*, utils::command::BotCommands,
};
use tokio::task;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "start the bot.")]
    Start,
    #[command(description = "display this help.")]
    Help,
    #[command(description = "get audio.")]
    Mp3(String),
    #[command(description = "get video.")]
    Mp4(String),
    #[command(description = "get audio as a voice message.")]
    Voice(String),
    #[command(description = "get video as a animated GIF.")]
    Gif(String),
    #[command(description = "delete trash messages.")]
    C,
}

async fn bot_init() -> Result<Bot, Box<dyn Error>> {
    debug!("Initializing the bot ...");
    let bot = Bot::from_env().set_api_url("http://telegram-api:8081".parse()?);
    Ok(bot)
}

pub async fn run() {
    loop {
        match bot_init().await {
            Ok(bot) => {
                info!("Connection has been established.");
                let db = database::initialize().await;
                dispatcher(bot, db).await;
            }
            Err(reason) => {
                error!("{}", reason);
            }
        }
        warn!("Could not establish connection. Trying again after 30 seconds.");
        sleep(30);
    }
}

async fn dispatcher(bot: Bot, db: Surreal<Db>) {
    Dispatcher::builder(bot, handler().await)
        .dependencies(dptree::deps![db])
        .default_handler(|upd| async move {
            warn!("Unhandled update: {:?}", upd);
        })
        .build()
        .dispatch()
        .await;
}

async fn handler() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(case![Command::Start].endpoint(start))
        .branch(case![Command::Help].endpoint(help))
        .branch(case![Command::Mp3(url)].endpoint(mp3))
        .branch(case![Command::Mp4(url)].endpoint(mp4))
        .branch(case![Command::Voice(url)].endpoint(voice))
        .branch(case![Command::Gif(url)].endpoint(gif))
        .branch(case![Command::C].endpoint(clear));

    let message_handler = Update::filter_message().branch(command_handler);

    return message_handler;
}

async fn start(bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    let username = getuser(&msg_from_user);
    let command_descriptions = Command::descriptions().to_string();
    send_and_remember_msg(&bot, chat_id, &db, &command_descriptions).await;
    database::intodb(chat_id, msg_id, &db).await?;
    info!("User @{} has /start'ed the bot.", username);
    Ok(())
}

async fn help(bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    let username = getuser(&msg_from_user);
    let command_descriptions = Command::descriptions().to_string();
    send_and_remember_msg(&bot, chat_id, &db, &command_descriptions).await;
    database::intodb(chat_id, msg_id, &db).await?;
    info!("User @{} asked for /help.", username);
    Ok(())
}

async fn mp3(url: String, bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let filetype = FileType::Mp3;
    process_request(url, filetype, &bot, msg_from_user, &db).await?;
    Ok(())
}

async fn mp4(url: String, bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let filetype = FileType::Mp4;
    process_request(url, filetype, &bot, msg_from_user, &db).await?;
    Ok(())
}

async fn voice(url: String, bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let filetype = FileType::Voice;
    process_request(url, filetype, &bot, msg_from_user, &db).await?;
    Ok(())
}

async fn gif(url: String, bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let filetype = FileType::Gif;
    process_request(url, filetype, &bot, msg_from_user, &db).await?;
    Ok(())
}

async fn clear(bot: Bot, msg_from_user: Message, db: Surreal<Db>) -> HandlerResult {
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    database::intodb(chat_id, msg_id, &db).await?;
    purge_trash_messages(chat_id, &db, &bot).await?;
    info!("User @{} has cleaned up the chat.", getuser(&msg_from_user));
    Ok(())
}

fn getuser(msg_from_user: &Message) -> String {
    let chatkind = &msg_from_user.chat.kind;
    let mut username: String = String::new();
    match chatkind {
        ChatKind::Private(chat) => {
            match &chat.username {
                None => username = "noname".to_string(),
                Some(name) => {
                    username = name.clone();
                }
            };
        }
        _ => {}
    }
    return username;
}

async fn purge_trash_messages(
    chat_id: ChatId,
    db: &Surreal<Db>,
    bot: &Bot,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let ids = database::get_trash_message_ids(chat_id, db).await?;
    for id in ids.into_iter() {
        trace!("Deleting Message ID {} from Chat {} ...", id.0, chat_id.0);
        match bot.delete_message(chat_id, id).await {
            Err(e) => {
                error!("Can't delete a message: {}", e);
            }
            _ => {}
        }
    }
    forget_deleted_messages(chat_id, db).await?;
    Ok(())
}

async fn process_request(
    url: String,
    filetype: FileType,
    bot: &Bot,
    msg_from_user: Message,
    db: &Surreal<Db>,
) -> HandlerResult {
    debug!("Processing request ...");
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    let username = getuser(&msg_from_user);
    database::intodb(chat_id, msg_id, &db).await?;
    if url_is_valid(&url) {
        send_and_remember_msg(&bot, chat_id, db, "Please wait ...").await;
        info!("User @{} asked for /{}.", &username, filetype.as_str());
        let downloads_result = match &filetype {
            FileType::Mp3 => task::spawn_blocking(move || pirate::mp3(url)).await,
            FileType::Mp4 => task::spawn_blocking(move || pirate::mp4(url)).await,
            FileType::Voice => task::spawn_blocking(move || pirate::ogg(url)).await,
            FileType::Gif => task::spawn_blocking(move || pirate::gif(url)).await,
        }?;

        match downloads_result {
            Err(error) => {
                warn!("{}", error);
                send_and_remember_msg(&bot, chat_id, &db, &error.to_string()).await;
            }
            Ok(files) => {
                if files.warnings.len() != 0 {
                    send_and_remember_msg(bot, chat_id, db, &files.warnings).await;
                }
                for path in files.paths.into_iter() {
                    let file = InputFile::file(&path);
                    let filename = path.file_name().unwrap().to_str().unwrap();
                    trace!("Sending '{}' to @{} ...", filename, &username);
                    match &filetype {
                        FileType::Mp3 => {
                            if let Err(error) = bot.send_audio(chat_id, file).await {
                                let error_text = format!("{}", error);
                                send_and_remember_msg(bot, chat_id, db, &error_text).await;
                            }
                        }
                        FileType::Mp4 => {
                            if let Err(error) = bot.send_video(chat_id, file).await {
                                let error_text = format!("{}", error);
                                send_and_remember_msg(bot, chat_id, db, &error_text).await;
                            }
                        }
                        FileType::Voice => {
                            if let Err(error) = bot.send_voice(chat_id, file).await {
                                let error_text = format!("{}", error);
                                send_and_remember_msg(bot, chat_id, db, &error_text).await;
                            }
                        }
                        FileType::Gif => {
                            if let Err(error) = bot.send_animation(chat_id, file).await {
                                let error_text = format!("{}", error);
                                send_and_remember_msg(bot, chat_id, db, &error_text).await;
                            }
                        }
                    }
                }
                info!("Files have been delivered to @{}.", &username);
                purge_trash_messages(chat_id, db, &bot).await?;
                cleanup(files.folder);
            }
        }
    } else {
        let ftype = filetype.as_str();
        let correct_usage = match &filetype {
            FileType::Voice => {
                format!("Correct usage:\n\n/voice https://valid_audio_url")
            }
            FileType::Gif => {
                format!("Correct usage:\n\n/{} https://valid_video_url", ftype)
            }
            _ => {
                format!("Correct usage:\n\n/{} https://valid_{}_url", ftype, ftype)
            }
        };
        send_and_remember_msg(&bot, chat_id, db, &correct_usage).await;
        info!("Reminded user @{} of a correct /{} usage.", username, ftype);
    }
    Ok(())
}

async fn send_and_remember_msg(bot: &Bot, chat_id: ChatId, db: &Surreal<Db>, text: &str) {
    let text_chunks = split_text(text);
    let mut chunk_index: usize = 0;
    debug!("Message chunks to send: {}.", text_chunks.len());
    for chunk in text_chunks {
        chunk_index += 1;
        trace!("Sending text message {} of length {} ...", chunk_index, chunk.len());
        let message_result = bot.send_message(chat_id, chunk).await;
        match message_result {
            Ok(message) => {
                if let Err(db_error) = database::intodb(chat_id, message.id, &db).await {
                    warn!("Failed to record a message into DB: {}", db_error);
                }
            }
            Err(msg_error) => {
                warn!("Failed to send message: {}", msg_error);
            }
        }
    }
}

// Telegram limits message length to 4096 chars. Thus the message is split into sendable chunks.
fn split_text(text: &str) -> Vec<String> {
    trace!("Splitting text into sendable chunks ...");
    if text.len() > 4096 {
        let stringvec = text.as_bytes()
            .chunks(4096)
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<Vec<String>>();
        return stringvec;
    } else {
        let stringvec = vec![text.to_string()];
        return stringvec;
    }
}
