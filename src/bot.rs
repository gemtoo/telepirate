use crate::{
    database::{self, forget_deleted_messages, get_last_message_id, TelepirateDbRecord},
    misc::*,
    pirate::{self, FileType},
};
use dptree::case;
use reqwest::Client as ReqwestClient;
use std::error::Error;
use std::time::Duration;
use surrealdb::{engine::remote::ws::Client as DbClient, Surreal};
use teloxide::types::ChatKind;
use teloxide::types::InputFile;
use teloxide::types::MessageId;
use teloxide::{dispatching::UpdateHandler, prelude::*, utils::command::BotCommands};
use tokio::task;
use uuid::Uuid;

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
    #[command(description = "delete trash messages.")]
    C,
}

fn bot_init() -> &'static Bot {
    debug!("Initializing the bot ...");
    let bot_token = match std::env::var("TELOXIDE_TOKEN") {
        Ok(token) => token,
        Err(e) => die(e.to_string()),
    };
    let client = ReqwestClient::builder()
        .timeout(Duration::from_secs(360))
        .build()
        .unwrap_or_else(|error| die(error.to_string()));
    let bot = Bot::with_client(bot_token, client)
        .set_api_url("http://telegram-api:8081".parse().unwrap());
    let boxed_bot = Box::new(bot);
    info!("Connection has been established.");
    return Box::leak(boxed_bot);
}

pub async fn run() {
    let bot = bot_init();
    let db = database::initialize().await;
    dispatcher(bot, db).await;
}

async fn dispatcher(bot: &'static Bot, db: &'static Surreal<DbClient>) {
    Dispatcher::builder(bot, handler().await)
        .dependencies(dptree::deps![db])
        .default_handler(|upd| async move {
            warn!("Unhandled update: {:?}", upd);
        })
        // Allows for concurrent requests from the same user.
        .distribution_function(|_| None::<std::convert::Infallible>)
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
        .branch(case![Command::C].endpoint(clear));

    let message_handler = Update::filter_message().branch(command_handler);

    return message_handler;
}

async fn start(
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    let username = getuser(&msg_from_user);
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} has /start'ed the bot.", username);
    send_and_remember_msg(bot, chat_id, db, &command_descriptions).await;
    database::intodb(chat_id, msg_id, db).await?;
    Ok(())
}

async fn help(
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let dbrecord = TelepirateDbRecord::from(msg_from_user);
    dbrecord.intodb(db).await.unwrap();
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} asked for /help.", dbrecord.username);
    send_and_remember_msg(bot, dbrecord.chat_id, db, &command_descriptions).await;
    database::intodb(dbrecord.chat_id, dbrecord.message_id, db).await?;
    Ok(())
}

async fn mp3(
    url: String,
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let filetype = FileType::Mp3;
    process_request(url, filetype, bot, msg_from_user, db).await?;
    Ok(())
}

async fn mp4(
    url: String,
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let filetype = FileType::Mp4;
    process_request(url, filetype, bot, msg_from_user, db).await?;
    Ok(())
}

async fn voice(
    url: String,
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let filetype = FileType::Voice;
    process_request(url, filetype, bot, msg_from_user, db).await?;
    Ok(())
}

async fn clear(
    bot: &'static Bot,
    msg_from_user: Message,
    db: &'static Surreal<DbClient>,
) -> HandlerResult {
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    database::intodb(chat_id, msg_id, db).await?;
    purge_trash_messages(chat_id, db, bot).await?;
    info!("User @{} has cleaned up the chat.", getuser(&msg_from_user));
    Ok(())
}

pub fn getuser(msg_from_user: &Message) -> String {
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
    db: &Surreal<DbClient>,
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

use tokio::sync::watch;
async fn process_request(
    url: String,
    filetype: FileType,
    bot: &Bot,
    msg_from_user: Message,
    db: &Surreal<DbClient>,
) -> HandlerResult {
    debug!("Processing request ...");
    let chat_id = msg_from_user.chat.id;
    let msg_id = msg_from_user.id;
    let username = getuser(&msg_from_user);
    info!("User @{} asked for /{}.", &username, filetype.as_str());
    database::intodb(chat_id, msg_id, db).await?;
    if url_is_valid(&url) {
        send_and_remember_msg(bot, chat_id, db, "Downloading... Please wait.").await;
        let last_message_id = get_last_message_id(chat_id, db).await?;
        // UUID is used because thats my choice.
        let download_id = Uuid::new_v4();
        // Channel is created because we need a way to exit a poller task after the request is done.
        // This can be gracefully done only by sending a signal through the channel.
        let (tx, rx) = watch::channel(false);
        let poller_handle = run_directory_size_poller_and_mesage_updater(
            rx,
            chat_id,
            last_message_id,
            download_id,
            filetype.clone(),
            bot.clone(),
        )
        .await;
        let downloads_result = match &filetype {
            FileType::Mp3 => task::spawn_blocking(move || pirate::mp3(url, &download_id)).await,
            FileType::Mp4 => task::spawn_blocking(move || pirate::mp4(url, &download_id)).await,
            FileType::Voice => task::spawn_blocking(move || pirate::ogg(url, &download_id)).await,
        }?;
        match downloads_result {
            Err(error) => {
                warn!("{}", error);
                send_and_remember_msg(bot, chat_id, db, &error.to_string()).await;
            }
            Ok(files) => {
                trace!(
                    "All files are ready. Finishing poller task for Chat ID {} ...",
                    chat_id
                );
                let _ = tx.send(true);
                poller_handle.await?;
                for path in files.paths.iter() {
                    send_file(path, &username, &filetype, bot, chat_id, db).await;
                }
                purge_trash_messages(chat_id, db, bot).await?;
                cleanup(files.folder);
            }
        }
    } else {
        let ftype = filetype.as_str();
        let correct_usage = match &filetype {
            FileType::Voice => {
                format!("Correct usage:\n\n/voice https://valid_audio_url")
            }
            _ => {
                format!("Correct usage:\n\n/{} https://valid_{}_url", ftype, ftype)
            }
        };
        send_and_remember_msg(bot, chat_id, db, &correct_usage).await;
        info!("Reminded user @{} of a correct /{} usage.", username, ftype);
    }
    Ok(())
}

use std::path::PathBuf;
async fn send_file(
    path: &PathBuf,
    username: &str,
    filetype: &FileType,
    bot: &Bot,
    chat_id: ChatId,
    db: &Surreal<DbClient>,
) {
    let file = InputFile::file(path);
    let filename = path.file_name().unwrap().to_str().unwrap();
    trace!("Sending '{}' to @{} ...", filename, &username);
    let sending_result;
    match filetype {
        FileType::Mp3 => {
            sending_result = bot.send_audio(chat_id, file).await;
        }
        FileType::Mp4 => {
            sending_result = bot.send_video(chat_id, file).await;
        }
        FileType::Voice => {
            sending_result = bot.send_voice(chat_id, file).await;
        }
    }
    match sending_result {
        Ok(_) => {
            info!(
                "File '{}' has been successfully delivered to @{}.",
                filename, username
            );
            return;
        }
        Err(error) => {
            let error_text = format!("File sending error: {}", error);
            warn!("{}", error_text);
            send_and_remember_msg(bot, chat_id, db, &error_text).await;
        }
    }
}

async fn send_and_remember_msg(bot: &Bot, chat_id: ChatId, db: &Surreal<DbClient>, text: &str) {
    let text_chunks = split_text(text);
    let mut chunk_index: usize = 0;
    trace!("Message chunks to send: {}.", text_chunks.len());
    for chunk in text_chunks {
        chunk_index += 1;
        trace!(
            "Sending text message {} of length {} ...",
            chunk_index,
            chunk.len()
        );
        let message_result = bot.send_message(chat_id, chunk).await;
        match message_result {
            Ok(message) => {
                if let Err(db_error) = database::intodb(chat_id, message.id, db).await {
                    warn!("Failed to record a message into DB: {}", db_error);
                }
            }
            Err(msg_error) => {
                warn!("Failed to send message: {}", msg_error);
            }
        }
    }
}

async fn run_directory_size_poller_and_mesage_updater(
    mut rx: watch::Receiver<bool>,
    chat_id: ChatId,
    last_message_id: MessageId,
    download_id: Uuid,
    filetype: FileType,
    bot: Bot,
) -> tokio::task::JoinHandle<()> {
    debug!(
        "Starting poller task for Chat ID {}, Download ID {} ...",
        chat_id, &download_id
    );
    let poller_handle = tokio::task::spawn({
        async move {
            let path_to_downloads = pirate::construct_destination_path(&download_id);
            // As long as we run in Docker as root, there should be no issues with this unwrap.
            std::fs::create_dir_all(&path_to_downloads).unwrap();
            let mut previous_update_text: String = String::new();
            // A loop that polls directory size and updates a last message in related chat.
            loop {
                tokio::select! {
                    // This case handles the main logic while rx is false.
                    _ = async {
                    sleep(1).await;
                    trace!("Polling data about Download ID {} ...", download_id);
                    let folder_data = FolderData::from(&path_to_downloads, filetype.clone());
                    trace!(
                        "Download ID {}. File count: {}. Current size: {}.",
                        &download_id, folder_data.file_count, folder_data.format_bytes_to_megabytes()
                    );
                    let update_text = format!(
                        "Downloading... Please wait.\nFiles downloaded: {}.\nTotal size of files: {}.",
                        folder_data.file_count,
                        folder_data.format_bytes_to_megabytes(),
                    );
                    // Telegram doesn't allow updating a message if content hasn't changed.
                    if update_text != previous_update_text {
                        previous_update_text = update_text.clone();
                        trace!("Updating a message in Chat ID {}, regarding Download ID {} ...", chat_id, &download_id);
                        if let Err(w) = bot.edit_message_text(chat_id, last_message_id, update_text).await {
                            warn!("Message in Chat ID {} related to Download ID {} wasn't updated: {}", chat_id, &download_id, w);
                        }
                    }
                    } => {}
                    // When a channel receives a change this means that a task should finalize.
                    _ = rx.changed() => {
                    if *rx.borrow() {
                        trace!("Poller task for Download ID {} done.", &download_id);
                        break;
                    }
                }
                }
            }
        }
    });
    return poller_handle;
}
