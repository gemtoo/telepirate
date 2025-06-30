# TelePirate Telegram Bot
## Download music and videos from anywhere via Telegram

#### What this bot can do?
This bot can help you extract files from the URL. At the moment of writing, TelePirate supports more than 1800 resources to download from. YouTube, SoundCloud, PornHub, to name a few. Entire playlists and channels can be downloaded. The bot can bypass age verification and some regional restrictions. Maximum file size it can send is 2 GB. Provide the bot with a URL and desired file type. For example, to download a song from a video clip on YouTube:
```
/mp3 https://youtu.be/XXYlFuWEuKI
```
To download a video clip of the best available quality:
```
/mp4 https://youtu.be/XXYlFuWEuKI
```
Use `/help` command to list all available commands.

#### Minimal system requirements:

Anything that runs Docker and Docker Compose. You don't need a VPS and/or public IP, the bot runs well in home networks that are behind NAT. Further instructions are tailored to installation on a generic Linux host. Basic command line knowledge is expected to follow the guide.

#### Deployment:

1. Create a Telegram bot and obtain [Telegram Bot API Token](https://core.telegram.org/bots#how-do-i-create-a-bot) from @BotFather.
2. Obtain [Telegram API ID](https://core.telegram.org/api/obtaining_api_id). This requires registering an app on Telegram's official website.
When registering an app, leave the URL field empty. In Platforms select 'Other' and specify brief description for the bot.
3. Install [Docker + Docker Compose for your OS](https://docs.docker.com/engine/install/).
4. Clone the repository and proceed inside:
```
git clone --depth 1 https://github.com/gemtoo/telepirate.git && cd telepirate
```
5. Inside of the repository create a `.env` file in your favorite text editor.
The file should contain only the 3 following lines:
```
TELOXIDE_TOKEN=your_bot's_token_from_step_1
TELEGRAM_API_ID=your_api_id_from_step_2
TELEGRAM_API_HASH=your_api_hash_from_step_2
```
6. Build and run the bot:
```
docker compose up -d --build
```
7. Test the bot by sending it some commands:
```
/start
/help
/c
```
### Troubleshooting notes
 The bot uses `yt-dlp` as the backend to download files. Sometimes YouTube pushes an update that breaks older versions of `yt-dlp`. In this case the bot starts throwing various `yt-dlp` errors in chat. First, consider updating `yt-dlp` inside the container:
```
docker exec -it telepirate /bin/sh -c 'pip install --break-system-packages -U "yt-dlp[default]"'
```
If updating didn't help, try rebuilding the container:
```
cd telepirate
docker compose down
git pull
docker compose build --no-cache
docker compose up -d --build
```
When downloading entire channels, check if the server has enough disk space, there is no way for the bot to prematurely know how much free space is needed to cache all pending downloads.
### Other notes
Due to Telegram's compliance with local laws, bots like this are getting censored and chats with them become unavailable for AppStore and Google Play users. It is recommended to run your own private instance of a bot to avoid censorship for as long as possible. This bot is deleting files after the request is finalized, leaving no evidence of copyright violations. The evidence exists only at the time of the request processing, which is fairly quick. It also strips off the metadata from files to make its work even more discreet. So that no metadata or hashsum matching checks will identify "illegal" files. TelePirate has been flawlessly running in DMCA compliant environment that is known to quickly shut down servers for working with pirated stuff.
