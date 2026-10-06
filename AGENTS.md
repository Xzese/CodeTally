# Repository guidance

## Build artifact cleanup

- Use `npm run tauri:build`, `npm run tauri:dev`, or `npm run tauri -- build/dev` for desktop builds. These wrappers clean temporary Cargo output while retaining the latest successful deliverables; see `docs/development.md`.
- After the task's required verification is complete, remove generated build and test artifacts that are no longer needed. Check `src-tauri/target/`, `dist/`, `src-tauri/gen/`, Vite caches, TypeScript `.tsbuildinfo` files, and temporary build/test directories created by the task. Direct Cargo commands leave output that needs explicit cleanup.
- Keep outputs needed by an active build, preview, test, or investigation, and retain deliverables or screenshots requested by the user. For intentional build retention, use `CODETALLY_KEEP_BUILD_ARTIFACTS=1` and explain what remains and why.
- Before deleting, verify that each path is generated, untracked, and belongs to this task or repository. Check for active consumers. Stop task-owned temporary preview/build processes when they are no longer needed; do not stop user-managed or shared processes.
- Scope cleanup to verified paths. Do not use broad `git clean` commands, sweep shared temporary directories, or delete other worktrees' outputs.
- Preserve source files, dependency installations and shared package caches, local application data, managed repository clones, `.repowise/`, tracked documentation assets, shared screenshots, and retained release deliverables in `artifacts/tauri/`.
- Report the artifacts removed, approximate disk space reclaimed, and any outputs intentionally retained. Do not rerun builds solely to verify a cleanup or documentation change.
