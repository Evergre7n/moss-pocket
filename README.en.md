# MOSS Pocket 1.0

[简体中文](README.md) · **English**

MOSS Pocket is a portable notebook for Windows. Start typing in a new note or record a voice memo while the thought is fresh. Notes and attachments are encrypted on your device.

[Download for Windows x64](https://github.com/Evergre7n/moss-pocket/releases/download/v1.0.0/MOSS-Pocket-1.0.0-windows-x64.zip) · [Release notes and SHA-256 checksums](https://github.com/Evergre7n/moss-pocket/releases/tag/v1.0.0)

Extract the ZIP and double-click `MOSS-Pocket.exe`. The Windows executable in 1.0.0 is unsigned. To upgrade, close the app, back up the entire `MOSS-Pocket-data` folder, then replace the old executable in its original location.

## How it works

The editor saves after you pause typing. Closing the window locks the notebook. MOSS Pocket runs natively with Rust and egui, using OpenGL with a DirectX 12 fallback. Videos are read from encrypted attachments on demand through a loopback connection on your computer.

The data folder lives alongside the executable. You can carry the notebook on a USB drive. The interface follows your system language, and you can switch between English and Chinese in Settings.

## Features

### Write and organize

- A new note opens with the cursor in the body. Leave the title blank or choose **Add title** later.
- The sidebar lists notes by modification time. Pin a note or add tags to find it again.
- Build a note from paragraphs, three levels of headings, bulleted or numbered lists, quotes, and dividers. Format text with bold, italic, underline, strikethrough, or color.
- Undo and redo edits across the note. To copy, cut, or paste a whole block, select it and use `Ctrl+C`, `Ctrl+X`, or `Ctrl+V`. After you copy something in another app, Paste uses the new clipboard content.

### Record and insert media

- **Record a voice memo.** Choose **Record** above the note, speak into the default microphone, then choose **Finish** to insert a playable clip. **Cancel** discards the take.
- A recording saves automatically at five minutes or 32 MiB of captured audio, whichever comes first. Switching notes, locking the notebook, or closing the window saves the recording to the note where it started.
- **Rename a recording.** Click the filename at the end of the audio row, such as `Recording.wav`. Type a name and press Enter to save or Esc to cancel. The extension stays in place. The player, saved attachments, and exports use the new name.
- **Insert a file.** Click where it belongs in the note, choose **Insert**, then **Image**, **Audio**, or **Video**, and confirm your selection. This also works in an empty note.
- Paste a screenshot or image file, or drag it into the window. Choose **View image** to see the full image. In the note, drag its lower-right corner to resize it or double-click the corner to reset its width.
- Play audio inline or choose **Open player** to seek through it. Choose **Play video** to watch a video in the note window and press Esc to return. Playback uses Windows Media Foundation. If a codec is unsupported, convert the file to an H.264 MP4 and insert it again.

### Search, export, and personalize

- Search the notebook by title, tag, or body text. Within a note, find and replace highlights matches. Press Enter to advance. **Replace all** counts as a single undo step.
- Deleted notes stay in the trash for 30 days.
- Export as Markdown with a media folder, HTML, Word, or PDF. PDF export uses Microsoft Edge's background printing and requires Edge on the computer.
- Choose a light, dark, or system theme. Adjust the window material (opaque, translucent, acrylic, or mica), tint, opacity, accent color, corner radius, fonts, line and paragraph spacing, interface language, and idle-lock interval.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | Create a note |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+B` / `Ctrl+I` / `Ctrl+U` | Bold / italic / underline |
| `Ctrl+Shift+X` | Strikethrough |
| `Ctrl+F` | Find in the current note, using selected text as the search term |
| `Ctrl+H` | Find and replace |
| `F3` / `Shift+F3` | Next / previous match |
| `Ctrl+L` | Lock now |
| `Alt+↑` / `Alt+↓` | Move the current block up / down |
| Enter | Split the block at the cursor |
| Backspace at the start of a block | Merge with the previous block |
| ↑ on the first line / ↓ on the last line | Move to the previous / next block |

The three-line menu to the left of a block lets you change its type, move it, copy it, insert a divider or media, or delete it.

## First launch and backups

1. Open `MOSS-Pocket.exe` and set a password for the notebook.
2. Store the recovery key somewhere safe. It is shown once and can unlock the notebook if you forget your password.
3. Start writing. The notebook lives in `MOSS-Pocket-data` next to the executable. The separate `MOSS-Pocket.settings.json` file stores preferences in plain text.

To back up or move a notebook, close the app and copy the entire `MOSS-Pocket-data` folder.

## Security

MOSS Pocket uses Argon2id for password-based key derivation and AES-256-GCM for authenticated encryption. Note count, media file sizes, and file modification times remain visible on disk. [Vault format and encryption design (Chinese)](docs/format.md)

Unlocked content is present in memory. Press `Ctrl+L` when you step away. The notebook locks after 15 minutes of inactivity by default. Use a strong password and consider full-disk encryption if others may have physical access to the computer.

## Build from source

The Windows x64 release uses Rust's `x86_64-pc-windows-gnullvm` toolchain, LLVM-MinGW (including the `windres` resource compiler), and CMake. The Opus decoder is linked statically.

```sh
cargo build --release --locked -p mp-app
```

The executable is written to `target/release/MOSS-Pocket.exe`. For a public build, use Python 3.11 or later to run `python tools/build_release.py`, which remaps local build paths. The source includes tools for generating third-party notices and packaging the release.

Run the standard test suite with:

```sh
cargo test --workspace
```

UI tests exercise the interface in memory. To also run device playback and background PDF export tests, use a Windows machine with an audio output device and Microsoft Edge.

```sh
cargo test --workspace --locked -- --include-ignored --test-threads=1
```

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/mp-core` | Encryption, vault format, note model, and editing logic |
| `crates/mp-app` | Desktop interface, media, export, and settings |
| `docs/format.md` | On-disk format and encryption design |

## License

The original MOSS Pocket code and documentation are available under the [MIT License](LICENSE). The MOSS marks and bundled fonts have separate [asset notices](docs/ASSET-NOTICES.md). The [third-party notices](THIRD-PARTY-NOTICES.md) list dependencies and their licenses.
