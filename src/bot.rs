use crate::{
    database::{self, TelepirateDbRecord},
    misc::*,
    pirate::{self, FileType},
};
use dptree::case;
use reqwest::Client as ReqwestClient;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;
use surrealdb::{engine::remote::ws::Client as DbClient, Surreal};
use teloxide::types::ChatKind;
use teloxide::types::InputFile;
use teloxide::types::MessageId;
use teloxide::{dispatching::UpdateHandler, prelude::*, utils::command::BotCommands};
use tokio::sync::watch;
use tokio::task;
use std::sync::Arc;

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

fn bot_init() -> Arc<Bot> {
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
        .set_api_url("http://telegram-bot-api:8081".parse().unwrap());
    
    info!("Connection has been established.");
    Arc::new(bot)
}

pub async fn run() {
    let bot = bot_init();
    let db = database::initialize().await;
    dispatcher(bot, db).await;
}

async fn dispatcher(bot: Arc<Bot>, db: Arc<Surreal<DbClient>>) {
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

#[derive(Clone)]
struct TelepirateSession {
    bot: Arc<Bot>,
    db: Arc<Surreal<DbClient>>,
}
impl TelepirateSession {
    fn from(bot: Arc<Bot>, db: Arc<Surreal<DbClient>>) -> Self {
        TelepirateSession { bot, db }
    }
    async fn send_and_remember_msg(&self, reference: &TelepirateDbRecord, text: &str) {
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
            let message_result = self.bot.send_message(reference.chat_id, chunk).await;
            match message_result {
                Ok(message) => {
                    let new_dbrecord =
                        TelepirateDbRecord::from(message, reference.request_id.clone());
                    new_dbrecord
                        .intodb(self.db.clone())
                        .await
                        .unwrap_or_else(|warning| warn!("Failed create a DB record: {}", warning));
                }
                Err(msg_error) => {
                    warn!("Failed to send message: {}", msg_error);
                }
            }
        }
    }
    async fn purge_all_trash_messages(
        &self,
        reference: &TelepirateDbRecord,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let ids = reference.msg_ids_fromdb_by_chat_id(self.db.clone()).await?;
        for id in ids.into_iter() {
            trace!(
                "Deleting Message ID {} from Chat {} ...",
                id.0,
                reference.chat_id
            );
            match self.bot.delete_message(reference.chat_id, id).await {
                Err(e) => {
                    error!("Can't delete a message: {}", e);
                }
                _ => {}
            }
        }
        reference.fromdb_delete_all_by_chat_id(self.db.clone()).await?;
        Ok(())
    }
    async fn purge_request_id_trash_messages(
        &self,
        reference: &TelepirateDbRecord,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let ids = reference.msg_ids_fromdb_by_request_id(self.db.clone()).await?;
        for id in ids.into_iter() {
            trace!(
                "Deleting Message ID {} from Chat {} ...",
                id.0,
                reference.chat_id
            );
            match self.bot.delete_message(reference.chat_id, id).await {
                Err(e) => {
                    error!("Can't delete a message: {}", e);
                }
                _ => {}
            }
        }
        reference.fromdb_delete_all_by_request_id(self.db.clone()).await?;
        Ok(())
    }
    async fn process_request(
        self,
        reference: TelepirateDbRecord,
        url: String,
        filetype: FileType,
    ) -> HandlerResult {
        debug!("Processing request ...");
        let chat_id = reference.chat_id;
        let username = &reference.username;
        info!("User @{} asked for /{}.", &username, filetype.as_str());
        reference.intodb(self.db.clone()).await?;
        if url_is_valid(&url) {
            self.send_and_remember_msg(&reference, "Downloading... Please wait.")
                .await;
            let last_message_id = reference.msg_id_fromdb_last(self.db.clone()).await?.unwrap();
            let download_id = reference.request_id.clone().to_string();
            // Channel is created because we need a way to exit a poller task after the request is done.
            // This can be gracefully done only by sending a signal through the channel.
            let (tx, rx) = watch::channel(false);
            let poller_handle = run_directory_size_poller_and_mesage_updater(
                rx,
                chat_id,
                last_message_id,
                download_id.clone(),
                filetype.clone(),
                self.bot.clone(),
            )
            .await;
            let downloads_result = match &filetype {
                FileType::Mp3 => task::spawn_blocking(move || pirate::mp3(url, download_id)).await,
                FileType::Mp4 => task::spawn_blocking(move || pirate::mp4(url, download_id)).await,
                FileType::Voice => {
                    task::spawn_blocking(move || pirate::ogg(url, download_id)).await
                }
            }?;
            match downloads_result {
                Err(error) => {
                    warn!("{}", error);
                    self.send_and_remember_msg(&reference, &error.to_string())
                        .await
                }
                Ok(files) => {
                    trace!(
                        "All files are ready. Finishing poller task for Chat ID {} ...",
                        chat_id
                    );
                    let _ = tx.send(true);
                    poller_handle.await?;
                    for path in files.paths.iter() {
                        self.clone().send_file(&reference, path, &filetype).await;
                    }
                    self.purge_request_id_trash_messages(&reference).await?;
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
            self.send_and_remember_msg(&reference, &correct_usage).await;
            info!("Reminded user @{} of a correct /{} usage.", username, ftype);
        }
        Ok(())
    }
    async fn send_file(self, reference: &TelepirateDbRecord, path: &PathBuf, filetype: &FileType) {
        let file = InputFile::file(path);
        let filename = path.file_name().unwrap().to_str().unwrap();
        let max_retries: usize = 10;
        for attempt in 1..=max_retries {
            let sending_result;
            trace!(
                "Attempt {}/{} at sending '{}' to @{} ...",
                attempt,
                max_retries,
                filename,
                reference.username
            );
            match filetype {
                FileType::Mp3 => {
                    sending_result = self.bot.send_audio(reference.chat_id, file.clone()).await;
                }
                FileType::Mp4 => {
                    sending_result = self.bot.send_video(reference.chat_id, file.clone()).await;
                }
                FileType::Voice => {
                    sending_result = self.bot.send_voice(reference.chat_id, file.clone()).await;
                }
            }
            match sending_result {
                Ok(_) => {
                    info!(
                        "File '{}' has been successfully delivered to @{}.",
                        filename, reference.username
                    );
                    return;
                }
                Err(error) => {
                    // In case of "Too many requests" error, cooldown is 10 seconds before sending another message.
                    sleep(10).await;
                    let error_text = format!(
                        "Attempt {}/{} at sending '{}'. File sending error: {}",
                        attempt, max_retries, filename, error
                    );
                    warn!("{}", error_text);
                    self.send_and_remember_msg(&reference, &error_text).await;
                }
            }
        }
    }
}

async fn start(
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user, request_id);
    dbrecord.intodb(db).await?;
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} has /start'ed the bot.", dbrecord.username);
    telepirate_session
        .send_and_remember_msg(&dbrecord, &command_descriptions)
        .await;
    Ok(())
}

use crate::database::RequestId;
async fn help(
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user, request_id);
    dbrecord.intodb(db).await?;
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} asked for /help.", dbrecord.username);
    telepirate_session
        .send_and_remember_msg(&dbrecord, &command_descriptions)
        .await;
    Ok(())
}

async fn mp3(
    url: String,
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user, request_id);
    let filetype = FileType::Mp3;
    telepirate_session
        .process_request(dbrecord, url, filetype)
        .await?;
    Ok(())
}

async fn mp4(
    url: String,
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user, request_id);
    let filetype = FileType::Mp4;
    telepirate_session
        .process_request(dbrecord, url, filetype)
        .await?;
    Ok(())
}

async fn voice(
    url: String,
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user, request_id);
    let filetype = FileType::Voice;
    telepirate_session
        .process_request(dbrecord, url, filetype)
        .await?;
    Ok(())
}

async fn clear(
    bot: Arc<Bot>,
    msg_from_user: Message,
    db: Arc<Surreal<DbClient>>,
) -> HandlerResult {
    let telepirate_session = TelepirateSession::from(bot, db.clone());
    let request_id = RequestId::new();
    let dbrecord = TelepirateDbRecord::from(msg_from_user.clone(), request_id);
    dbrecord.intodb(db).await?;
    telepirate_session
        .purge_all_trash_messages(&dbrecord)
        .await?;
    info!("User @{} has cleaned up the chat.", dbrecord.username);
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

async fn run_directory_size_poller_and_mesage_updater(
    mut rx: watch::Receiver<bool>,
    chat_id: ChatId,
    last_message_id: MessageId,
    download_id: String,
    filetype: FileType,
    bot: Arc<Bot>,
) -> tokio::task::JoinHandle<()> {
    debug!(
        "Starting poller task for Chat ID {}, Download ID {} ...",
        chat_id, &download_id
    );
    let poller_handle = tokio::task::spawn({
        async move {
            let path_to_downloads = pirate::construct_destination_path(download_id.clone());
            // As long as we run in Docker as root, there should be no issues with this unwrap.
            std::fs::create_dir_all(&path_to_downloads).unwrap();
            let mut previous_update_text: String = String::new();
            // A loop that polls directory size and updates a last message in related chat.
            loop {
                tokio::select! {
                    // This case handles the main logic while rx is false.
                    _ = async {
                    // Update less frequently to avoid getting cooled down by Telegram.
                    sleep(5).await;
                    trace!("Polling data about Download ID {} ...", download_id);
                    let folder_data = FolderData::from(&path_to_downloads, filetype.clone());
                    trace!(
                        "Download ID {}. File count: {}. Current size: {}.",
                        &download_id, folder_data.file_count, folder_data.format_bytes_to_megabytes()
                    );
                    let update_text = format!(
                        "Downloading... Please wait.\nFiles to send: {}.\nTotal size of files: {}.",
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
