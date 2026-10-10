# Hosting online play yourself

Playing together needs two services that Pipit does not run for you:

- An **introduction server**, which turns a code into a connection. Pipit uses
  the public PeerJS server by default. It only passes the first few messages
  between the two browsers; the game traffic goes directly from one to the other.
- A **relay** (a TURN server), only when a direct connection is impossible
  because both players sit behind strict NATs (some mobile carriers, some
  offices). Without one, those pairs simply fail to connect. Relayed traffic
  passes through the server, so this costs bandwidth.

Both can run on one small machine with [`server/docker-compose.yml`](../server/docker-compose.yml):

```bash
cd server
cp turnserver.conf.example turnserver.conf   # set the user, password, realm and public IP
docker compose up -d
```

Then, in Pipit's *Play together* dialog, open *Advanced: connection settings*
on every player's device:

| Setting | Value |
|---------|-------|
| Introduction server | `https://your.domain:9000/pipit` |
| Relay (TURN) URL | `turn:your.domain:3478` |
| Relay username / credential | the user and password from `turnserver.conf` |

The website is served over HTTPS, so browsers only accept the introduction
server over TLS: put it behind a reverse proxy with a certificate (Caddy does
this with one line), or run the PeerJS server with its own certificate. The relay
can stay plain `turn:`; the data channel itself is always encrypted.

Everyone in a session must use the same introduction server: that is where the
code is looked up.
