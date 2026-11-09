<p align="center"><img src="assets/icon/icon-256.png" width="96" alt=""></p>

# Todo & Notes

A desktop app for todo lists and Markdown notes, built with [Rune](https://github.com/anomalyco/rune), a GPU-rendered Rust UI framework. It syncs through the [simple-to-do](https://github.com/tf200/simple-to-do) server.

- Lists and tasks with due dates, details and a done section
- Markdown notes with live preview, task checkboxes and pinning
- Edits show at once and save in the background; nothing waits on the network
- Stays signed in (short-lived access tokens, rotating refresh tokens in the system keychain)
- Updates itself from this repo's releases

## Install

Download from the [latest release](https://github.com/tf200/todo-notes/releases/latest):

- **Windows:** `TodoNotes-win-Setup.exe`. It installs for your user only (no admin) and adds Start menu and desktop shortcuts. The installer isn't code-signed yet, so Windows may show "Windows protected your PC": choose **More info → Run anyway**.
- **Linux:** `TodoNotes.AppImage`. Make it executable (`chmod +x`) and run it.
- **macOS:** coming soon.

The app checks for new versions when it starts and every few hours, downloads them in the background, and offers to restart into them.

## Build

Needs Rust (the version in `rust-toolchain.toml`). On Windows, build inside the Visual Studio 2022 developer environment (`vcvars64.bat`).

```sh
cargo run                                      # against https://todo.loket.site
TODO_API_URL=http://localhost:8090 cargo run   # against a local server
cargo test                                     # unit tests
cargo run -- --snap out.png --scene todos      # headless screenshot of a scene (see src/ui.rs)
```

## Release

Releases are built and published from a developer machine with [Velopack](https://velopack.io):

```powershell
.\release.ps1 0.2.0          # Windows + Linux (via WSL): build, pack, tag, upload to GitHub Releases
.\release.ps1 0.2.0 -Local   # pack into .\releases only, to try an update first
```

```sh
./release-mac.sh 0.2.0       # on a Mac, adds the macOS build to the same release
```

To test an update before publishing it, pack two versions with `-Local`, install the older one, and start it with `TODO_UPDATE_SOURCE` set to `releases\win`.

App icons are generated from `assets/icon/mark.png` by `python scripts/make_icons.py`.

## License

[MIT](LICENSE)
