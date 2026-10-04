# CodeTally

A local desktop dashboard for your GitHub repositories. Track source and test line history, browse repository metadata, and see recent pull requests and issues in one place.

Built with Tauri, React, TypeScript, Rust, and SQLite. Currently built and verified on macOS.

![CodeTally dashboard using fictional example repositories and activity](docs/screenshots/dashboard-overview.png)

## Features

- Portfolio totals and source/test line history.
- Sortable repositories with language, visibility, activity, and recent growth.
- Repository detail pages and filterable pull-request and issue feeds.
- A local Kanban board with notes, priorities, and ticket placement.
- Menu bar metrics, automatic refreshes, and progress tracking for imports.

## Get started

Install `gh` (GitHub CLI), `git`, and `tokei`, and make sure they are on `PATH`. To run from source, you also need Node.js/npm, Rust/Cargo, and the native prerequisites for Tauri 2.

```sh
gh auth login
npm install
npm run tauri:dev
```

On first launch, choose **Import repositories**. CodeTally discovers repositories owned by your account and visible organizations, including private repositories. The initial import can take time while repositories are cloned and their history is sampled.

Use **Refresh Tickets** for personal PR and issue activity, or **Settings → Background & refresh → Force Refresh** for a full synchronization. Adjust automatic refresh intervals in Settings. Archived repositories are excluded from the active portfolio, and forks are excluded from line totals by default.

## Privacy

GitHub access is read-only; Kanban planning changes stay local. CodeTally uses your existing GitHub CLI authentication. It does not ask for or store a GitHub token. Metadata, line history, and repository clones—including private source code—are stored locally in the operating system's application-data directory. GitHub requests still require a network connection.

## Development

```sh
npm test             # Frontend tests
npm run build        # TypeScript checks and frontend build
npm run tauri:build  # Native app bundle
cargo test --manifest-path src-tauri/Cargo.toml
```

## More information

- [User guide](docs/user-guide.md): menu bar controls, Kanban, refresh behavior, line-count rules, and troubleshooting.
- [Development guide](docs/development.md): architecture, build outputs, CI, and releases.

No software license has been selected yet. Public visibility does not grant permission to redistribute or reuse the source.
