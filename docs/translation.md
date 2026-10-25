# Lyric translation

Aoide for Linux translates lyrics that aren't in your language with your own
[LibreTranslate](https://github.com/LibreTranslate/LibreTranslate) server
(AGPL-3.0, Argos Translate's models). The lyrics go to that server and
nowhere else. The app works out the lyrics' language itself and only offers
Translate when they aren't in yours.

## On the server

LibreTranslate publishes Docker images. With an NVIDIA card, use the CUDA
one; without, the plain `latest` image works, more slowly.

```yaml
# compose.yaml
services:
  libretranslate:
    image: libretranslate/libretranslate:latest-cuda   # or :latest without a GPU
    restart: unless-stopped
    ports:
      - "127.0.0.1:5000:5000"     # only the reverse proxy reaches it
    command: --disable-web-ui
    environment:
      - LT_LOAD_ONLY=en,ja,ko,fr,es,de,pt,it   # the languages you listen to
      - LT_UPDATE_MODELS=true
      - LT_API_KEYS=true
      - LT_REQ_LIMIT=0                         # with API keys: no key, no answer
    volumes:
      - ./libretranslate/db:/app/db
      # The CUDA image runs as root; the plain one keeps its models in
      # /home/libretranslate/.local instead.
      - ./libretranslate/models:/root/.local
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu]
```

Then make a key for the app:

```
docker compose exec libretranslate ltmanage keys add 120
```

Put it behind the same HTTPS reverse proxy as Jellyfin (for example
`translate.example.net`). An address on your home network works over plain
HTTP, as Jellyfin's does, but HTTPS is better anywhere else.

Nothing here talks to a hosted translation service.

## In the app

Settings → Lyrics → Translation Server: type the address and Save. The app
asks the server which languages it has and says so. Then paste the key under
API Key (it is kept in the system keyring and never shown again). On a song
whose lyrics aren't in your language, Translate appears above the lyrics.

What is sent: the lyric lines, your language and the key. Nothing about you,
the song or the computer.
