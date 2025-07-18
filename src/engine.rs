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
use tracing::{debug, error, info, warn}; // Added warn for completeness
use url::Url;

use crate::{
    database::{self, DbRecord},
    misc::die,
    pirate::FileType,
    task::{HasTaskId, TaskState, TrackedMessage},
};

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
}

/// Initializes and configures the Telegram bot instance
#[tracing::instrument]
fn bot_init() -> Bot {
    debug!("Initializing bot client...");
    let bot_token = std::env::var("TELOXIDE_TOKEN").unwrap_or_else(|e| die(e.to_string()));

    // Configure HTTP client with extended timeout for file operations
    let client = ReqwestClient::builder()
        .timeout(Duration::from_secs(360))
        .build()
        .unwrap_or_else(|error| die(error.to_string()));

    // URL of the Dockerized Telegram Bot API
    let api_url = "http://telegram-bot-api:8081"
        .parse()
        .unwrap_or_else(|_| die("Invalid API URL".to_string()));

    let bot = Bot::with_client(bot_token, client).set_api_url(api_url);

    info!("Bot client initialized successfully");
    bot
}

/// Main entry point for bot execution
#[tracing::instrument]
pub async fn run() {
    let bot = bot_init();
    let db = database::db_init().await;

    // Configure visible bot commands (exclude /start from UI)
    let mut commands = Command::bot_commands().to_vec();
    commands.retain(|c| c.command != "/start");
    bot.set_my_commands(commands)
        .scope(BotCommandScope::Default)
        .await
        .unwrap_or_else(|_| die("Failed to set bot commands.".to_string()));

    // Start event dispatcher
    dispatcher(bot, db).await;
}

/// Configures update dispatcher with handlers
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

/// Generates media type selection keyboard
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

/// Handles callback queries from inline keyboards
#[tracing::instrument(skip_all)]
async fn callback_handler(
    bot: Bot,
    callback_query: CallbackQuery,
    db: Surreal<DbClient>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Processing callback query...");
    let message = callback_query.regular_message().unwrap();

    // Retrieve task states for current chat
    let task_states_from_db =
        TaskState::select_from_db_by_chat_id(message.chat.id, db.clone()).await?;

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
    let media_type = match FileType::from_callback_data(data) {
        Some(mt) => mt,
        None => {
            bot.answer_callback_query(callback_query.id)
                .text("Invalid selection")
                .await?;
            return Ok(());
        }
    };

    bot.answer_callback_query(callback_query.id.clone()).await?;

    let chat_id = message.chat.id;
    let text = format!("Selected {}. Please send the content URL.", media_type);

    // Transition task state from New to WaitingForUrl
    let mut task_state_that_is_new = states_new[0].clone();
    let mut_task_session = task_state_that_is_new.as_mut_session();
    mut_task_session.set_media_type(media_type);
    let task_state_that_is_waiting_for_url = task_state_that_is_new.to_waiting_for_url();

    // Update database state
    task_state_that_is_waiting_for_url
        .delete_by_task_id(db.clone())
        .await?;
    task_state_that_is_waiting_for_url
        .intodb(db.clone())
        .await?;

    // Update message with next instructions
    if let Err(e) = bot.edit_message_text(chat_id, message.id, &text).await {
        error!("Message edit failed: {}", e);
    }

    Ok(())
}

/// Handles incoming messages and commands
#[tracing::instrument(skip_all)]
async fn message_handler(
    bot: Bot,
    msg_from_user: Message,
    me: Me,
    db: Surreal<DbClient>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let chat_id = msg_from_user.chat.id;

    // Process text commands
    if let Some(text) = msg_from_user.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(Command::Clear) => {
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;
                let task_session = task_state.session();
                task_session
                    .remember_related_message(&msg_from_user, db.clone())
                    .await?;

                // Retrieve clearable tasks (New/WaitingForUrl states)
                let task_states = TaskState::select_from_db_by_chat_id(chat_id, db.clone()).await?;
                let clearable_tasks: Vec<TaskState> = task_states
                    .into_iter()
                    .filter(|s| matches!(s, TaskState::New(_) | TaskState::WaitingForUrl(_)))
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
            Ok(Command::Start) | Ok(Command::Ask) => {
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;
                let task_session = task_state.session();
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
            Err(_) => {
                // Err represents an unknown command, it can be any message from user, in this case just track it
                // Initialize new task session
                let task_state = TaskState::try_from(&msg_from_user)?;
                task_state.intodb(db.clone()).await?;
                let task_session = task_state.session();
                task_session
                    .remember_related_message(&msg_from_user, db.clone())
                    .await?;
            }
        }
    }

    // Check for URL input in WaitingForUrl state
    let task_states = TaskState::select_from_db_by_chat_id(chat_id, db.clone())
        .await
        .expect("Database query failed");
    let waiting_states: Vec<TaskState> = task_states
        .into_iter()
        .filter(|s| matches!(s, TaskState::WaitingForUrl(_)))
        .collect();

    match waiting_states.len() {
        0 => {} // No URL expected
        1.. => {
            let task_state = waiting_states[0].clone();
            let task_session = task_state.session();

            // Track user's message containing URL
            task_session
                .remember_related_message(&msg_from_user, db.clone())
                .await?;

            // Process URL input
            if let Some(raw_url) = msg_from_user.text() {
                match Url::parse(raw_url) {
                    Ok(url) => {
                        // Mark task as running
                        let running_state = task_state.clone().to_running();
                        task_state.delete_by_task_id(db.clone()).await.ok();
                        running_state.intodb(db.clone()).await.ok();
                        task_session
                            .process_request(
                                url.to_string(),
                                task_session.media_type.clone().unwrap(),
                                bot.clone(),
                                db.clone(),
                            )
                            .await?;
                        // Mark task as successful
                        let final_state = running_state.clone().to_success();
                        running_state.delete_by_task_id(db.clone()).await.ok();
                        final_state.intodb(db.clone()).await.ok();
                    }
                    Err(e) => {
                        let text = format!("Invalid URL: {}. Please try again", e);
                        task_session
                            .send_and_remember_msg(&text, bot.clone(), db.clone())
                            .await?;
                    }
                }
            }
            return Ok(());
        }
    }
    Ok(())
}
