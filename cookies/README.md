# Support for Netscape formatted cookies.txt file

Some resources might require age verification, being signed in to confirm you are not a bot, CloudFlare 403 bot protection error, etcetera. For this case, place Netscape formatted `cookies.txt` file right in this folder, then do
```
docker compose up -d
```
The bot should be able to understand that `cookies.txt` is in place and act accordingly.