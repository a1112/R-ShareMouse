# Desktop Frontend Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move the active Tauri frontend out of `other/figma-ui` into `apps/rshare-desktop-frontend` and remove the obsolete static desktop UI.

**Architecture:** `apps/rshare-desktop` remains the Tauri/Rust desktop shell. `apps/rshare-desktop-frontend` becomes the only active React/Vite frontend used by Tauri build and dev commands. Historical plans may keep old paths, but runnable configs and scripts must use the new structure.

**Tech Stack:** Tauri 2, Rust workspace, React/Vite frontend, npm scripts, existing PowerShell/Unix build scripts.

---

### Task 1: Move Frontend Tree

**Files:**
- Move: `other/figma-ui` -> `apps/rshare-desktop-frontend`
- Delete: `apps/rshare-desktop/ui`
- Delete: `other` if it becomes empty

**Steps:**
1. Verify `apps/rshare-desktop-frontend` does not already exist.
2. Move `other/figma-ui` to `apps/rshare-desktop-frontend`.
3. Delete `apps/rshare-desktop/ui`.
4. Delete `other` if empty.

### Task 2: Update Runtime References

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/tauri.conf.json`
- Modify: `bin/macos/build.sh`

**Steps:**
1. Update Tauri build/dev commands to use `../../rshare-desktop-frontend` from `apps/rshare-desktop/src-tauri`.
2. Update `frontendDist` to `../../rshare-desktop-frontend/dist`.
3. Update macOS frontend build path to `$REPO_ROOT/apps/rshare-desktop-frontend`.

### Task 3: Verify

**Commands:**
- `npm test --prefix apps/rshare-desktop-frontend`
- `npm run build --prefix apps/rshare-desktop-frontend`
- `cargo check -p rshare-desktop`
- `rg -n "other/figma-ui|apps/rshare-desktop/ui|rshare-desktop/ui|figma-ui" apps bin Cargo.toml README.md AGENTS.md CLAUDE.md`

Expected: tests/build/check pass, and active code/scripts no longer reference the old frontend paths.
