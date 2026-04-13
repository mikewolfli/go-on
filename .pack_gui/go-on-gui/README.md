# go-on GUI

Desktop console for go-on built with Tauri 2 + Vue + Vite.

## Scope

- Independent project under GUI/
- Does not modify existing go-on source code
- Controls go-on process externally (start/stop/restart)
- Visualizes health, monitoring, logs, and AI usage snapshots
- Monitors Zed/VS Code integration interfaces (process, endpoint, addon)
- Supports zh-CN and en-US UI language switching
- Supports tray resident mode and mini floating console

## Commands

- `npm install` (or `pnpm install`)
- `npm run dev`
- `npm run build`
- `npm run tauri dev`
- `npm run tauri build`

## Notes

- Default polling intervals:
  - Monitoring/Status: 2s
  - Logs: 1s
- Main window closes to tray by default.
- Tray menu supports Show/Start/Stop/Restart/Mini Console/Quit.
- Built executable output:
  - `GUI/src-tauri/target/release/go-on-gui.exe`
