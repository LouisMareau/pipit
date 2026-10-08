# Deploying the web app

The app is static files: anything that serves `build/web` works. Two free options.

## GitHub Pages (built in)

`.github/workflows/deploy.yml` builds and publishes on every push to `main`.

1. Push the repository to GitHub.
2. In the repository: **Settings → Pages → Build and deployment → Source: GitHub Actions**.
3. Push to `main` (or run the workflow from the Actions tab).

The app appears at `https://<user>.github.io/<repo>/`. The workflow sets `PIPIT_BASE`
to `/<repo>/` so asset paths and the PWA scope match. With a custom domain or a
`<user>.github.io` repository, set `PIPIT_BASE` to `/` instead.

## Cloudflare Pages

Connect the repository in the Cloudflare dashboard with:

- Build command: `npm ci --prefix web && npm run build --prefix web`
- Build output directory: `build/web`
- Environment: Rust and `wasm-bindgen-cli` are needed, so use the Cloudflare
  "V2" build system with `RUST_VERSION` set, and add
  `cargo install wasm-bindgen-cli --version <version from Cargo.lock>` to the build
  command. Alternatively build in GitHub Actions and publish `build/web` with
  `cloudflare/wrangler-action` (`wrangler pages deploy build/web`).

Cloudflare serves from the domain root, so no `PIPIT_BASE` is needed.

## What to keep in mind

- ROMs are never part of the deployment. Users add their own files; everything stays
  in their browser storage.
- The service worker precaches the app, so it keeps working offline after the first
  visit and updates itself on the next load after a deploy.
- A custom domain is nicer for installation prompts and bookmarks, but not required.
