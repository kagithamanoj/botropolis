# Hosting Botropolis

The Rust server ships as one Docker image: the release binary, the
static web UI, the agent specs, and the model registry. This page
covers what the image needs, how the two modes differ, where state
lives, and how to put it on Fly.io.

## The image

```sh
docker build -t botropolis .
```

The build is multi-stage: the `rust:1-bookworm` stage compiles the
workspace, and the `debian:bookworm-slim` runtime stage runs as a
non-root `botropolis` user. The image carries no Python and no
model weights.

## Smoke test

```sh
docker run --rm -p 8000:8000 \
  -e BOTROPOLIS_MASTER_KEY="$(openssl rand -hex 32)" \
  botropolis
curl http://127.0.0.1:8000/health
```

Expect `{"status":"ok","agents":20}`. The container starts in
hosted mode, so `/ask` without a team key answers 401; sign up
first (see below), then call with the key.

## Environment variables

| Variable | Required | What it does |
| --- | --- | --- |
| `BOTROPOLIS_HOSTED` | In the image, set to `1` | Hosted mode: requests without a team key or session get 401 |
| `BOTROPOLIS_MASTER_KEY` | Yes, for BYOK | 64 hex characters, or any passphrase (its SHA-256 is the key). Without it, storing provider keys is refused with 503 |
| `PORT` | No (default `8000`) | Port the server listens on |
| `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GOOGLE_API_KEY` | No | Platform fallback keys, used only when a team has no stored key for that provider |
| `BOTROPOLIS_FREE_REQUESTS_PER_MONTH` | No (default `1000`) | Free-plan request quota |
| `BOTROPOLIS_FREE_TOKENS_PER_MONTH` | No (default `1000000`) | Free-plan token quota |

Never put a real provider key or the master key in the image or
in `fly.toml`. Set them as secrets on the host (below).

## Single-tenant vs hosted mode

- Single-tenant mode is the local default: `botropolis serve`
  with no `--hosted` flag. Every request uses the default tenant,
  which keeps the old paths under `botropolis/data/`. No quota is
  enforced.
- Hosted mode is `botropolis serve --hosted`, or
  `BOTROPOLIS_HOSTED=1`. A request must carry an
  `X-Botropolis-Key` header or a `botropolis_session` cookie, or
  it gets 401. Each team gets its own memory, ledger, run state,
  notebook, and usage log under
  `botropolis/data/tenants/<team_id>/`, and the free-plan quota
  applies to `/ask` and `/team`.

## Where state lives

Everything the server writes goes under `botropolis/data/`:

| Path | Contents |
| --- | --- |
| `botropolis/data/platform.db` | Teams, API keys, sessions, encrypted provider keys |
| `botropolis/data/tenants/<team_id>/` | That team's memory, ledger, runs, notebook, usage log |
| `botropolis/data/memory.db`, `usage.jsonl`, `runs/` | Default tenant state (single-tenant mode) |

Mount a volume on `botropolis/data` (in the image, that is
`/app/botropolis/data`). Without one, a redeploy wipes every team
account and every tenant's memory. The volume holds secrets
(password hashes, key hashes, encrypted provider keys), so keep
it private to the app.

## Deploying on Fly.io

Fly.io is the suggested host: one small machine with a persistent
volume is enough to start, and `flyctl` handles the TLS
certificate.

### 1. Install flyctl and sign in

```sh
curl -L https://fly.io/install.sh | sh
fly auth login
```

### 2. Create the app

From the repo root:

```sh
fly launch --name botropolis --region dfw --no-deploy
```

Accept the defaults. This writes a `fly.toml`; make sure it has
an HTTP service on port 8000 and a mount for state:

```toml
app = "botropolis"
primary_region = "dfw"

[build]

[http_service]
  internal_port = 8000
  force_https = true
  auto_stop_machines = false
  auto_start_machines = true
  min_machines_running = 1

[[mounts]]
  source = "botropolis_data"
  destination = "/app/botropolis/data"
```

Set `auto_stop_machines = false` and `min_machines_running = 1`:
the server holds per-team state in local files, so it must stay
up on the machine that owns the volume.

### 3. Create the volume

```sh
fly volumes create botropolis_data --size 1 --region dfw
```

One gigabyte is plenty to start; grow it later with
`fly volumes extend`.

### 4. Set the secrets

```sh
fly secrets set BOTROPOLIS_MASTER_KEY="$(openssl rand -hex 32)"
```

Add platform fallback keys only if the platform should pay for
model calls when a team has no stored key:

```sh
fly secrets set OPENAI_API_KEY="sk-..." ANTHROPIC_API_KEY="sk-ant-..."
```

### 5. Deploy

```sh
fly deploy
fly status
fly logs
```

`fly status` should show one machine running, and the logs
should show `Botropolis serving on http://0.0.0.0:8000`.

### 6. Try it

```sh
APP=https://botropolis.fly.dev
curl $APP/health
curl -X POST $APP/signup \
  -H 'content-type: application/json' \
  -d '{"email": "team@example.com", "password": "a long password", "name": "Acme"}'
```

The signup response carries a `session_token`. Use it as a
cookie, or create an API key and call with the header:

```sh
curl -X POST $APP/teams/api-keys \
  -H "cookie: botropolis_session=<session_token>" \
  -H 'content-type: application/json' \
  -d '{"name": "ci"}'
curl -X POST $APP/ask \
  -H "x-botropolis-key: <key>" \
  -H 'content-type: application/json' \
  -d '{"request": "research the history of the abacus"}'
```

### 7. Updates and rollback

```sh
fly deploy        # new image from main
fly releases      # list releases
fly releases rollback <version>   # back to a known-good release
```

### 8. Backups

The volume is the whole platform. Copy it off the machine on a
schedule, for example with `fly ssh sftp` to pull
`/app/botropolis/data/platform.db` and the `tenants/` directory
to local storage. There is no managed backup for a Fly volume.

## What is not built yet

Billing (Stripe) is a later phase: there is no way to charge a
team today. There is also no team dashboard UI and no member
invites; teams are one account with one or more API keys.
