## 🎉 First preview of VPS Files

A fast, lightweight, Finder-style file manager for your computer **and** your servers.

### 📥 Which file do I need?

| | File | Install |
|---|---|---|
| 🪟 **Windows 10 / 11** | [**VPS-Files-setup-x64.exe**](https://github.com/FarrukhDevloper/Files/releases/download/v0.1.0/VPS-Files-setup-x64.exe) · [.msi](https://github.com/FarrukhDevloper/Files/releases/download/v0.1.0/VPS-Files-x64.msi) | Double-click and follow the wizard |
| 🐧 **Ubuntu / Debian** | [**VPS-Files-amd64.deb**](https://github.com/FarrukhDevloper/Files/releases/download/v0.1.0/VPS-Files-amd64.deb) | `sudo apt install ./VPS-Files-amd64.deb` |
| 🎩 **Fedora / openSUSE** | [**VPS-Files-x86_64.rpm**](https://github.com/FarrukhDevloper/Files/releases/download/v0.1.0/VPS-Files-x86_64.rpm) | `sudo dnf install ./VPS-Files-x86_64.rpm` |
| 📦 **Any Linux** | [**VPS-Files-x86_64.AppImage**](https://github.com/FarrukhDevloper/Files/releases/download/v0.1.0/VPS-Files-x86_64.AppImage) | `chmod +x` it, then run it |

> ⏳ The installers are built by GitHub Actions and appear below a few minutes after this release is published.

### ✨ Highlights

- 🔑 Servers from `~/.ssh/config` (or added in the app) appear in the sidebar and **connect automatically**
- 🏎️ **SFTP at OpenSSH speed**, with instant navigation (cache, prefetch, optimistic updates)
- 🔁 Copy and move **local ↔ VPS ↔ VPS**, with Replace / Merge / Keep Both / Skip
- ✏️ **Edit remote files in your own apps**; saves sync back with conflict detection
- 🎬 **Stream video and audio** from servers with seeking, in-app or in VLC/mpv
- 🖼️ Smart **thumbnails** (EXIF, scaled decode, server-side ffmpeg) in uniform frames
- 🌿 **Git**: branch, ahead/behind, status badges, history, *Open on GitHub*
- 📊 **Live server stats** on Home and 💻 **Open in Terminal** in any folder
- 🗂️ List / Icon / Column views, tabs, 🪟 dual pane, 👀 Quick Look, 🌓 light & dark
- 🪶 ~14 MB · ~135 MB RAM · ~0 % CPU when idle

### ⚠️ Preview notes

- 🐧 Linux is tested daily against real servers.
- 🪟 The Windows build is produced by CI but **hasn't been tested on real hardware yet**. Please [report anything odd](https://github.com/FarrukhDevloper/Files/issues).
- 🔏 The installers aren't code-signed yet, so Windows SmartScreen may ask you to confirm ("More info → Run anyway").
- 🍎 macOS, ☁️ cloud drives (Google Drive, iCloud, Yandex Disk …) and more are on the [roadmap](https://github.com/FarrukhDevloper/Files/blob/main/docs/ROADMAP.md).
