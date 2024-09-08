use crate::misc::{self, url_is_valid};
use crate::{
    database::{self, forget_deleted_messages, get_last_message_id},
    misc::{cleanup, sleep, split_text},
    pirate::{self, FileType},
};
use std::path::PathBuf;
use dptree::case;
use reqwest::Client;
use std::error::Error;
use std::time::Duration;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use teloxide::types::ChatKind;
use teloxide::types::InputFile;
use teloxide::types::MessageId;
use teloxide::{dispatching::UpdateHandler, prelude::*, utils::command::BotCommands};
use tokio::task;
use uuid::Uuid;
use tokio::sync::watch;

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

// Static lifetime is OK because the Bot should live as long as the program.
async fn bot_init() -> Result<&'static Bot, Box<dyn Error>> {
    debug!("Initializing the bot ...");
    let bot_token = std::env::var("TELOXIDE_TOKEN")?;
    let client = Client::builder()
        .timeout(Duration::from_secs(360))
        .build()?;
    let bot = Bot::with_client(bot_token, client).set_api_url("http://telegram-api:8081".parse()?);
    let boxed_bot = Box::new(bot);
    Ok(Box::leak(boxed_bot))
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

async fn dispatcher(bot: &'static Bot, db: &'static Surreal<Db>) {
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

async fn start(bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, None, bot, db);
    let username = telepirate_request.username();
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} has /start'ed the bot.", username);
    telepirate_request.send_and_remember_msg(&command_descriptions).await;
    database::intodb(&telepirate_request).await?;
    Ok(())
}

async fn help(bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, None, bot, db);
    let username = telepirate_request.username();
    let command_descriptions = Command::descriptions().to_string();
    info!("User @{} asked for /help.", username);
    telepirate_request.send_and_remember_msg(&command_descriptions).await;
    database::intodb(&telepirate_request).await?;
    Ok(())
}

async fn mp3(url: String, bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, Some(FileType::Mp3), bot, db);
    telepirate_request.process_request(url).await?;
    Ok(())
}

async fn mp4(url: String, bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, Some(FileType::Mp4), bot, db);
    telepirate_request.process_request(url).await?;
    Ok(())
}

async fn voice(url: String, bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, Some(FileType::Voice), bot, db);
    telepirate_request.process_request(url).await?;
    Ok(())
}

async fn clear(bot: &'static Bot, msg_from_user: Message, db: &'static Surreal<Db>) -> HandlerResult {
    let telepirate_request = TelepirateRequest::from(msg_from_user, None, bot, db);
    let chat_id = telepirate_request.chat_id();
    database::intodb(&telepirate_request).await?;
    purge_trash_messages(chat_id, &db, &bot).await?;
    info!("User @{} has cleaned up the chat.", telepirate_request.username());
    Ok(())
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

#[derive(Debug, Clone)]
pub struct TelepirateRequest {
    msg_from_user: Message,
    filetype: Option<FileType>,
    request_id: Uuid,
    bot: &'static Bot,
    db: &'static Surreal<Db>,
}

impl TelepirateRequest {
    pub fn from(msg_from_user: Message, filetype: Option<FileType>, bot: &'static Bot, db: &'static Surreal<Db>) -> Self {
        let telepirate_request = TelepirateRequest {
            msg_from_user,
            filetype,
            request_id: Uuid::new_v4(),
            bot,
            db
        };
        return telepirate_request;
    }
    fn username(&self) -> String {
        let msg_from_user = &self.msg_from_user;
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
    pub fn chat_id(&self) -> ChatId {
        self.msg_from_user.chat.id
    }
    pub fn msg_id(&self) -> MessageId {
        self.msg_from_user.id
    }
    pub fn db(&self) -> &'static Surreal<Db> {
        self.db
    }
    fn filetype(&self) -> Option<FileType> {
        self.filetype.clone()
    }
    fn bot(&self) -> &'static Bot {
        self.bot
    }
    pub fn request_id(&self) -> String {
        self.request_id.to_string()
    }
    async fn send_and_remember_msg(&self, text: &str) {
        let bot = self.bot();
        let db = self.db();
        let chat_id = self.chat_id();
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
               if let Err(db_error) = database::intodb(self).await {
                        warn!("Failed to record a message into DB: {}", db_error);
                    }
                }
                Err(msg_error) => {
                    warn!("Failed to send message: {}", msg_error);
                }
            }
        }
    }
    async fn process_request(self, url: String) -> HandlerResult {
        info!(
                    "User @{} asked for /{}. Processing request ID {} from Chat ID {} ...",
                    self.username(),
                    self.filetype().unwrap().as_str(),
                    self.request_id(),
                    self.chat_id()
        );
        let request_id = self.request_id();
        let chat_id = self.chat_id();
        let msg_id = self.msg_id();
        let username = self.username();
        let db = self.db();
        let filetype = self.filetype().unwrap();
        let bot = self.bot();
        database::intodb(&self).await?;
        if url_is_valid(&url) {
            self.send_and_remember_msg("Downloading... Please wait.").await;
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
                    self.send_and_remember_msg(&error.to_string()).await;
                }
                Ok(files) => {
                    trace!(
                        "All files are ready. Finishing poller task for Chat ID {} ...",
                        chat_id
                    );
                    let _ = tx.send(true);
                    poller_handle.await?;
                    for path in files.paths.iter() {
                        self.send_file(path).await;
                    }
                    // TODO remove unwrap below
                    dbg!(database::get_trash_messages_of_current_request_id(&self).await.unwrap());
                    //purge_trash_messages(chat_id, db, &bot).await?;
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
            self.send_and_remember_msg(&correct_usage).await;
            info!("Reminded user @{} of a correct /{} usage.", username, ftype);
        }
        Ok(())
    }
    async fn send_file(
        &self,
        path: &PathBuf,
    ) {
        let bot = self.bot();
        let filetype = self.filetype().unwrap();
        let username = self.username();
        let chat_id = self.chat_id();
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
                self.send_and_remember_msg(&error_text).await;
            }
        }
    }
}

async fn run_directory_size_poller_and_mesage_updater(
    mut rx: watch::Receiver<bool>,
    chat_id: ChatId,
    last_message_id: MessageId,
    download_id: Uuid,
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
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    trace!("Polling data about Download ID {} ...", download_id);
                    // Unwrap because this directory should already exist. Triggering previous unwrap woudn't get us here.
                    let current_directory_size_bytes = misc::get_directory_size(&path_to_downloads).unwrap();
                    let current_directory_size_megabytes_formatted = format!(
                        "{:.2} MB",
                        current_directory_size_bytes as f64 / (1024.0 * 1024.0)
                    );
                    trace!("Download ID {}. Current size: {}.", &download_id, &current_directory_size_megabytes_formatted);
                    let update_text = format!(
                        "Downloading ... Please wait.\nCurrent size of downloads folder: {}.",
                        &current_directory_size_megabytes_formatted
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
