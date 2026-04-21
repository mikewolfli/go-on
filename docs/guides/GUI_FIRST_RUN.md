# GUI First-Run Guide

This guide covers the shortest closed loop for the desktop GUI after the BLUE36 three-tab refactor.

## 1. Open The GUI

- Start the backend first, or let the GUI configure and start it.
- Launch the desktop app from `GUI/` with `npm run tauri dev` during development.

## 2. Follow The First-Run Guide

- Use the header action `Get Started` / `快速上手` to open the built-in onboarding dialog.
- The guide walks through three main areas:
  - `Monitor`: health, logs, AI usage, editor integrations.
  - `Config`: executable path, protocol mode, providers, backend operations.
  - `Chat`: session-based conversation against the local runtime.

## 3. Recommended First Pass

1. Open `Config -> Setup` and run a quick initialization or doctor check.
2. Open `Config -> Config` and confirm executable path, working directory, and protocol mode.
3. Open `Config -> Providers` and verify provider/model selection if chat requests depend on external credentials.
4. Open `Monitor -> Dashboard` or `Monitor -> Health Breakdown` and confirm runtime health.
5. Open `Chat` and send a real request to `/v1/chat/completions`.

## 4. Notes

- Theme switching now cycles through `default`, `meadow`, `ink`, `wuxia`, and `kitty`.
- The onboarding dialog can be reopened at any time from the app header.
- If the runtime is intentionally managed outside GUI, enable monitor-only mode in `Config`.