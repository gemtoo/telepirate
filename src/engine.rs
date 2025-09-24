use std::error::Error;
use std::time::Duration;

use reqwest::Client as ReqwestClient;
use surrealdb::{Surreal, engine::remote::ws::Client as DbClient};
use teloxide::{
    prelude::*,
    types::BotCommandScope,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Me},
    utils::command::BotCommands,
};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::{
    database::{self, DbRecord},
    misc::die,
    task::{
        cancellation::{CancellationRegistry, TASK_REGISTRY},
        mediatype::MediaType,
        state::TaskState,
        traits::{HasTaskId, Task},
    },
    trackedmessage::TrackedMessage,
};

use tokio_util::sync::CancellationToken;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Supported commands
#[derive(BotCommands, Clone, Default)]
#[command(rename_rule = "lowercase")]
enum Command {
    /// Start the bot
    #[default]
    Start,
    /// Ask for media
    Ask,
    /// Clear the chat
    Clear,
    /// Stop all running tasks
    Stop,
}

// Initializes and configures the Telegram bot instance
#[tracing::instrument]
fn bot_init() -> Bot {
    debug!("Initializing bot client ...");
    let bot_token = std::env::var("TELOXIDE_TOKEN").unwrap_or_else(|e| die(e.to_string()));

    // Configure HTTP client with extended timeout for file operations
    let client = ReqwestClient::builder()
        .timeout(Duration::from_secs(360))
        .build()
        .unwrap_or_else(|error| die(error.to_string()));

    // URL of the Dockerized Telegram Bot API
    let api_url = "http://telegram-bot-api:8081"
        .parse()
        .unwrap_or_else(|_| die("Invalid API URL.".to_string()));

    let bot = Bot::with_client(bot_token, client).set_api_url(api_url);

    info!("Bot client initialized successfully.");
    bot
}

// Main entry point for bot execution
#[tracing::instrument]
pub async fn run() {
    let bot = bot_init();
    let db = database::db_init().await;
    // On boot there can't be Running tasks. Finalize all Running tasks as Failed.
    if let Ok(task_states) = TaskState::from_db_all(db.clone()).await {
        let tasks: Vec<TaskState> = task_states
            .into_iter()
            .filter(|s| matches!(s, TaskState::Running(_)))
            .collect();
        for mut task in tasks {
            task.to_failure(db.clone()).await
        }
    }
    // Initialize cancellation registry.
    CancellationRegistry::new();
    // Configure visible bot commands (exclude /start from UI)
    let mut commands = Command::bot_commands().to_vec();
    commands.retain(|c| c.command != "/start");
    bot.set_my_commands(commands)
        .scope(BotCommandScope::Default)
        .await
        .unwrap_or_else(|_| die("Failed to set bot commands.".to_string()));
    // let bot_clone = bot.clone();
    // let db_clone = db.clone();
    // tokio::task::spawn(async move {
    //     finalize_interrupted_tasks(bot_clone, db_clone)
    //         .await
    //         .unwrap();
    // });
    // Start event dispatcher
    dispatcher(bot, db).await;
}

// Configures update dispatcher with handlers
#[tracing::instrument(skip_all)]
async fn dispatcher(bot: Bot, db: Surreal<DbClient>) {
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler))
        .branch(Update::filter_callback_query().endpoint(callback_handler));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![db])
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build()
        .dispatch()
        .await;
}

// Generates media type selection keyboard
fn make_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("Audio", "Audio"),
            InlineKeyboardButton::callback("Video", "Video"),
        ],
        vec![InlineKeyboardButton::callback(
            "Audio as voice message",
            "Audio as voice message",
        )],
    ])
}

// Handles callback queries from inline keyboards
#[tracing::instrument(skip_all, fields(user_id = %callback_query.from.id))]
async fn callback_handler(
    bot: Bot,
    callback_query: CallbackQuery,
    db: Surreal<DbClient>,
) -> HandlerResult {
    let username = match callback_query.from.username.clone() {
        Some(username) => username,
        None => "noname".to_string(),
    };
    let message = callback_query.regular_message().unwrap();

    // Retrieve task states for current chat
    let task_states_from_db = TaskState::from_db_by_chat_id(message.chat.id, db.clone()).await?;

    // Filter for 'New' state tasks
    let states_new: Vec<TaskState> = task_states_from_db
        .into_iter()
        .filter(|c| matches!(c, TaskState::New(_)))
        .collect();

    // Validate callback data exists
    let data = match callback_query.data.as_deref() {
        Some(data) => data,
        None => return Ok(()),
    };

    // Map callback data to media type
    let media_type = match MediaType::from_callback_data(data) {
        Some(media_type) => {
            info!("User @{} selected {}.", username, media_type);
            media_type
        }
        None => {
            bot.answer_callback_query(callback_query.id)
                .text("Invalid selection")
                .await?;
            return Ok(());
        }
    };

    bot.answer_callback_query(callback_query.id.clone()).await?;

    let chat_id = message.chat.id;
    let text = format!("Selected {media_type}. Please send the content URL.");

    // Transition task state from New to WaitingForUrl
    let mut task_state = states_new[0].clone();
    task_state.to_waiting_for_url(media_type, db.clone()).await;

    // Update message with next instructions
    if let Err(e) = bot.edit_message_text(chat_id, message.id, &text).await {
        error!("Message edit failed: {}", e);
    }

    Ok(())
}

// Handles incoming messages and commands, the unwrap in tracing macro is safe because all messages
// that we process are from the real users with real IDs.
#[tracing::instrument(skip_all, fields(user_id = %msg_from_user.from.clone().unwrap().id))]
async fn message_handler(
    bot: Bot,
    msg_from_user: Message,
    me: Me,
    db: Surreal<DbClient>,
) -> HandlerResult {
    let username = match msg_from_user.from.clone().unwrap().username {
        Some(username) => username,
        None => "noname".to_string(),
    };
    let chat_id = msg_from_user.chat.id;

    // Process text commands
    if let Some(text) = msg_from_user.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(Command::Start) | Ok(Command::Ask) => {
                info!("User @{username} did /start or /ask ...");
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;
                let task_session = task_state.get_inner_task_simple().unwrap();
                task_session
                    .remember_related_message(&msg_from_user, db.clone())
                    .await?;

                // Present media type selection
                let keyboard = make_keyboard();
                let text = "Select content type:";
                task_session
                    .send_and_remember_msg_with_keyboard(text, keyboard, bot.clone(), db.clone())
                    .await?;
                return Ok(());
            }
            Ok(Command::Stop) => {
                info!("User @{username} did /stop ...");
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;

                let task_session = task_state.get_inner_task_simple().unwrap();
                task_session
                    .remember_related_message(&msg_from_user, db.clone())
                    .await?;
                // Retrieve stoppable tasks (Running state)
                let task_states = TaskState::from_db_by_chat_id(chat_id, db.clone()).await?;
                let stoppable_tasks: Vec<TaskState> = task_states
                    .into_iter()
                    .filter(|s| matches!(s, TaskState::Running(_)))
                    .collect();
                for task in stoppable_tasks {
                    TASK_REGISTRY.cancel_task(task.task_id());
                }
                return Ok(());
            }
            Ok(Command::Clear) => {
                info!("User @{username} did /clear ...");
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;

                let task_session = task_state.get_inner_task_simple().unwrap();
                task_session
                    .remember_related_message(&msg_from_user, db.clone())
                    .await?;
                // Retrieve clearable tasks (New/WaitingForUrl states)
                let task_states = TaskState::from_db_by_chat_id(chat_id, db.clone()).await?;
                let clearable_tasks: Vec<TaskState> = task_states
                    .into_iter()
                    .filter(|s| {
                        matches!(
                            s,
                            TaskState::New(_) | TaskState::WaitingForUrl(_) | TaskState::Failure(_)
                        )
                    })
                    .collect();
                // Purge task-related messages and data
                for task in clearable_tasks {
                    let task_id = task.task_id();
                    // Delete tracked messages
                    let messages = TrackedMessage::from_db_by_task_id(task_id, db.clone()).await?;
                    for msg in messages {
                        msg.delete_by_task_id(db.clone()).await?;
                        bot.delete_message(chat_id, msg.message_id).await.ok();
                    }

                    // Delete task state
                    task.delete_by_task_id(db.clone()).await?;
                }
                return Ok(());
            }
            Err(_) => {
                // Err represents an unknown command, it can be any message from user, for example a random thanks or a URL that we wait
                info!("User @{username} said '{}'.", msg_from_user.text().unwrap());
                // Check for URL input in WaitingForUrl state
                let task_states = TaskState::from_db_by_chat_id(chat_id, db.clone()).await?;
                let waiting_states: Vec<TaskState> = task_states
                    .into_iter()
                    .filter(|s| matches!(s, TaskState::WaitingForUrl(_)))
                    .collect();

                match waiting_states.len() {
                    // 0 means we don't wait for any URL, in this initialize task session and just track user's message
                    0 => {
                        // If a message doesn't contain audio, then track the message. This is done to avoid deletion of audios from the user.
                        if let None = msg_from_user.audio() {
                        let task_state = TaskState::try_from(&msg_from_user)?;
                        task_state.intodb(db.clone()).await?;
                        let task_session = task_state.get_inner_task_simple().unwrap();
                        task_session
                            .remember_related_message(&msg_from_user, db.clone())
                            .await?;
                        }
                    }
                    // 1 or more means we expect a URL
                    1.. => {
                        let mut task_state = waiting_states[0].clone();
                        let task_download_non_running =
                            task_state.get_inner_task_download().unwrap();

                        // Track user's message
                        task_download_non_running
                            .remember_related_message(&msg_from_user, db.clone())
                            .await?;

                        // Process URL input
                        if let Some(raw_url) = msg_from_user.text() {
                            match Url::parse(raw_url) {
                                Ok(url) => {
                                    // Create cancellation token for task, in case it needs to be stopped
                                    let task_cancellation_token = CancellationToken::new();
                                    // Mark task as running
                                    task_state
                                        .to_running(url, db.clone(), task_cancellation_token)
                                        .await;
                                    let task_download_running =
                                        task_state.get_inner_task_download().unwrap();
                                    let request_processing_result = task_download_running
                                        .process_request(bot.clone(), db.clone())
                                        .await;
                                    match request_processing_result {
                                        Ok(_) => {
                                            // Mark task as successful
                                            task_state.to_success(db.clone()).await;
                                        }
                                        Err(_) => {
                                            // Mark task as failed
                                            task_state.to_failure(db.clone()).await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    let text = format!("Invalid URL: {e}. Please try again");
                                    task_download_non_running
                                        .send_and_remember_msg(&text, bot.clone(), db.clone())
                                        .await?;
                                }
                            }
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    Ok(())
}

// This is a dangerous function. It does correctly resume tasks but it can result in a dead loop
// where some running task crashes a program, then crashes it again and again when entering this function on boot
// enable at your own risk
// #[tracing::instrument(skip_all)]
// async fn finalize_interrupted_tasks(bot: Bot, db: Surreal<DbClient>) -> HandlerResult {
//     // Filter only Running tasks
//     let task_states = TaskState::from_db_all(db.clone()).await?;
//     let running_states: Vec<TaskState> = task_states
//         .into_iter()
//         .filter(|s| matches!(s, TaskState::Running(_)))
//         .collect();

//     info!(
//         "Found {} interrupted tasks to finalize.",
//         running_states.len()
//     );

//     // Use JoinSet to manage and track all tasks
//     let mut join_set = tokio::task::JoinSet::new();

//     for mut task_state in running_states {
//         // Separate variable needed to move it into async move.
//         let bot_clone = bot.clone();
//         let db_clone = db.clone();
//         let finalizer_span = tracing::info_span!(
//             "task_finalizer",
//             task_id = ?task_state.task_id(),
//         );
//         join_set.spawn(
//             async move {
//                 // Safe unwrap due to prior filtering
//                 let task_download = task_state
//                     .get_inner_task_download()
//                     .expect("Filtered state should contain TaskDownload");

//                 // Process with error logging
//                 match task_download
//                     .process_request(bot_clone, db_clone.clone())
//                     .await
//                 {
//                     Ok(_) => {
//                         info!("Successfully finalized task: {:?}.", task_state);
//                         task_state.to_success(db_clone).await;
//                     }
//                     Err(e) => {
//                         warn!("Failed to finalize task {:?}: {}.", task_state, e);
//                         task_state.to_failure(db_clone).await;
//                     }
//                 }
//             }
//             .instrument(finalizer_span),
//         );
//     }

//     // Wait for all tasks to complete with no time limit
//     while let Some(res) = join_set.join_next().await {
//         match res {
//             Ok(_) => {} // Individual task results already logged
//             Err(e) => error!("Task finalization panicked: {}", e),
//         }
//     }

//     info!("All interrupted tasks finalized");
//     Ok(())
// }
