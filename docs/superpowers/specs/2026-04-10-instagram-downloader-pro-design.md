# Instagram Downloader Pro — Design Specification

## Context

Build a professional, payware-grade Windows desktop application for downloading Instagram media (Reels, 4K Videos, Photos, Carousels, Stories, Highlights) at the highest possible bitrate and resolution from the CDN. Zero-compression extraction is the core differentiator — users get pristine DASH-muxed video at maximum quality.

**Tech stack:** Tauri v2 (Rust backend) + React 18 (TypeScript, Vite, Tailwind CSS, Shadcn/ui, GSAP)
**Target platform:** Windows 11 (x64)
**Licensing:** None — payware-grade means quality, not commercial distribution

---

## 1. System Architecture

```
┌─────────────────────────────────────┐
│         React Frontend (Vite)       │
│  TypeScript + Tailwind + Shadcn/ui  │
│  GSAP animations                    │
├──────────┬──────────┬───────────────┤
│ Commands │ Events   │ Channels      │
│ (req/res)│(lifecycle)│ (progress)   │
├──────────┴──────────┴───────────────┤
│         Tauri v2 IPC Layer          │
├─────────────────────────────────────┤
│         Rust Backend Engine         │
│  ┌───────────┐ ┌──────────────────┐ │
│  │ Extractor │ │ DownloadManager  │ │
│  │ Engine    │ │ (Queue+Workers)  │ │
│  ├───────────┤ ├──────────────────┤ │
│  │ AuthMgr   │ │ FfmpegManager   │ │
│  └───────────┘ └──────────────────┘ │
│  ┌──────────────────────────────────┐│
│  │ SQLite (queue, history, config) ││
│  └──────────────────────────────────┘│
└─────────────────────────────────────┘
          │
          ▼
    ffmpeg.exe (managed child process)
```

**IPC model:** Hybrid — Tauri Commands for request/response, Events for lifecycle notifications (queued, started, complete, error, paused), Channels for real-time byte-level download progress streaming.

---

## 2. Project Structure

```
instagram-downloader-pro/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   ├── icons/
│   └── src/
│       ├── main.rs
│       ├── lib.rs
│       ├── commands/
│       │   ├── mod.rs
│       │   ├── download.rs
│       │   ├── extract.rs
│       │   ├── auth.rs
│       │   ├── settings.rs
│       │   └── ffmpeg.rs
│       ├── extractor/
│       │   ├── mod.rs            # Extractor trait
│       │   ├── graphql.rs        # GraphQL API (primary)
│       │   ├── page_parser.rs    # HTML/JSON fallback
│       │   ├── types.rs          # MediaPost, MediaItem, MediaType
│       │   └── dash.rs           # DASH manifest parser
│       ├── download/
│       │   ├── mod.rs
│       │   ├── manager.rs        # Queue orchestration, concurrency
│       │   ├── worker.rs         # Per-download tokio task
│       │   ├── muxer.rs          # FFmpeg child process wrapper
│       │   └── types.rs          # DownloadTask, DownloadStatus, Progress
│       ├── ffmpeg/
│       │   ├── mod.rs
│       │   ├── manager.rs        # Fetch, verify, extract, update
│       │   └── version.rs        # GitHub API version check
│       ├── auth/
│       │   ├── mod.rs
│       │   ├── session.rs        # Cookie extraction, validation
│       │   └── keyring.rs        # OS credential storage
│       ├── db/
│       │   ├── mod.rs
│       │   ├── migrations.rs
│       │   └── models.rs
│       └── utils/
│           ├── mod.rs
│           ├── http.rs           # reqwest client with cookie injection
│           ├── proxy.rs          # Proxy/UA rotation, exponential backoff
│           ├── fs.rs             # Path sanitization, output dir resolution
│           ├── logger.rs         # tracing → AppData/logs/
│           └── error.rs          # App-wide error types
├── src/
│   ├── index.html
│   ├── main.tsx
│   ├── App.tsx
│   ├── components/
│   │   ├── ui/                   # Shadcn components
│   │   ├── layout/
│   │   │   ├── Sidebar.tsx
│   │   │   ├── TitleBar.tsx      # Custom draggable titlebar
│   │   │   └── MainLayout.tsx
│   │   ├── download/
│   │   │   ├── UrlInput.tsx
│   │   │   ├── MediaPreview.tsx
│   │   │   ├── QueueItem.tsx
│   │   │   ├── QueueList.tsx
│   │   │   └── ProgressBar.tsx
│   │   ├── auth/
│   │   │   └── LoginPrompt.tsx
│   │   └── settings/
│   │       └── SettingsPanel.tsx
│   ├── hooks/
│   │   ├── useDownloadQueue.ts
│   │   ├── useExtractor.ts
│   │   └── useAuth.ts
│   ├── stores/
│   │   └── appStore.ts           # Zustand
│   ├── lib/
│   │   ├── tauri.ts              # Typed IPC wrappers
│   │   └── types.ts              # TS types mirroring Rust
│   ├── animations/
│   │   ├── index.ts              # GSAP timeline registry
│   │   ├── transitions.ts        # Page/layout transitions
│   │   ├── progress.ts           # Progress bar interpolations
│   │   └── queue.ts              # Queue item enter/exit/reorder
│   └── styles/
│       └── globals.css
├── package.json
├── vite.config.ts
├── tailwind.config.ts
└── tsconfig.json
```

---

## 3. Rust Backend — Extractor Engine

### Extractor Trait

```rust
#[async_trait]
pub trait Extractor: Send + Sync {
    fn can_handle(&self, url: &str) -> bool;
    async fn fetch_metadata(
        &self, url: &str, session: &Session, http: &HttpClient,
    ) -> Result<MediaPost, ExtractorError>;
    fn priority(&self) -> u8; // lower wins
}
```

**Implementations:**
- `GraphQLExtractor` (priority 0) — uses Instagram's `graphql/query` with `doc_id` params. Handles posts, reels, carousels, stories, highlights via different query hashes.
- `PageParserExtractor` (priority 10) — fallback. Parses `<script type="application/ld+json">`, `window.__additionalDataLoaded`, and `window._sharedData` from page HTML.

### Core Types

```rust
pub enum MediaType { Photo, Video, Reel, Carousel, Story, Highlight }

pub struct MediaPost {
    pub id: String,
    pub shortcode: String,
    pub media_type: MediaType,
    pub owner_username: String,
    pub caption: Option<String>,
    pub timestamp: i64,
    pub items: Vec<MediaItem>,
}

pub struct MediaItem {
    pub id: String,
    pub media_type: MediaType,
    pub video_url: Option<String>,
    pub dash_manifest: Option<String>,
    pub photo_url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
}
```

### DASH Manifest Parser (`dash.rs`)
Parses MPD XML from Instagram's DASH manifests to extract the highest-bitrate video and audio `AdaptationSet`/`Representation`. Returns separate video and audio CDN URLs for FFmpeg muxing.

---

## 4. FFmpeg Manager

**Location:** `src-tauri/src/ffmpeg/`

### Responsibilities
1. **Bootstrap:** On app launch, check `{app_data}/ffmpeg/ffmpeg.exe` exists
2. **Download:** If missing/outdated, fetch from `BtbN/FFmpeg-Builds` GitHub releases (`ffmpeg-master-latest-win64-gpl.zip`)
3. **Verify:** SHA256 checksum from release assets
4. **Extract:** `zip` crate to `{app_data}/ffmpeg/`
5. **Version track:** `{app_data}/ffmpeg/version.json` with release tag + checksum
6. **Update check:** Compare local version.json against GitHub API latest release

### Mux Command
```
ffmpeg -i video.mp4 -i audio.m4a -c copy -movflags +faststart output.mp4
```
Executed via `tokio::process::Command` with:
- Absolute path to `ffmpeg.exe` (no PATH lookup, no shell)
- Arguments as array (no string interpolation)
- stdout/stderr piped for progress parsing (`time=` regex for progress %)
- Timeout watchdog (kill process if hung > 5 minutes per file)
- Exit code validation (0 = success, else propagate error)

---

## 5. Download Pipeline

### DownloadManager (`download/manager.rs`)
- Owns a `tokio::sync::Semaphore` for concurrency control (default: 3, configurable)
- Spawns `Worker` tokio tasks per download
- Manages pause/resume via `tokio::sync::watch` channels per task
- Persists all state transitions to SQLite
- On app startup: recovers queue from SQLite, resumes `downloading` tasks as `queued`

### Worker (`download/worker.rs`)
Per-download lifecycle:
1. Determine download strategy from `MediaItem`:
   - **Photo:** Single GET → save directly
   - **Video (direct URL):** Single GET → save (lower quality)
   - **Video (DASH):** Parse manifest → GET highest video stream + GET highest audio stream → FFmpeg mux → save
2. Stream download with `reqwest` response bytes, updating `Channel<DownloadProgress>`
3. On DASH: download video + audio to temp files, then invoke `Muxer`
4. Emit lifecycle events via Tauri `app_handle.emit()`

### Muxer (`download/muxer.rs`)
Wraps FFmpeg child process execution:
- Takes video temp path + audio temp path → output path
- Parses stderr for `time=` progress
- Cleans up temp files on success or failure
- Returns `Result<PathBuf, MuxError>`

---

## 6. Authentication

### Flow
1. `open_login_window` → opens Tauri `WebviewWindow` to `https://www.instagram.com/accounts/login/`
2. User authenticates naturally (handles 2FA, CAPTCHAs transparently)
3. `on_navigation` handler detects successful login (URL matches `instagram.com` homepage)
4. Extract cookies from WebView: `sessionid`, `csrftoken`, `ds_user_id`, `ig_did`
5. Store in Windows Credential Manager via `keyring` crate
6. Close login window, emit `session:updated` event

### Session Management
- On app launch: read keyring → validate session via API hit → set auth state
- Validation endpoint: `GET /api/v1/users/web_profile_info/?username=instagram` with session cookies
- 200 = valid, 401/403 = expired → emit `session:expired`, prompt re-login
- `reqwest::Client` built with `cookie_store` populated from keyring values

### Security
- Cookies NEVER stored in SQLite, files, or logs
- `tracing` filter excludes `sessionid`/`csrftoken` values from all log output
- Keyring entries scoped to app identifier (`instagram-downloader-pro`)

---

## 7. IPC Command Reference

### Commands (Frontend → Backend, request/response)

| Command | Params | Returns |
|---------|--------|---------|
| `resolve_url` | `url: String` | `MediaPost` |
| `enqueue_download` | `post: MediaPost, quality: QualityPref` | `{ task_id: String, channel: Channel<DownloadProgress> }` |
| `enqueue_batch` | `posts: Vec<MediaPost>, quality: QualityPref` | `Vec<{ task_id, channel }>` |
| `pause_download` | `task_id: String` | `bool` |
| `resume_download` | `task_id: String` | `bool` |
| `cancel_download` | `task_id: String` | `bool` |
| `get_queue_state` | — | `Vec<DownloadTask>` |
| `clear_completed` | — | `u32` |
| `open_login_window` | — | `bool` |
| `check_session` | — | `SessionStatus` |
| `logout` | — | `bool` |
| `get_settings` | — | `AppSettings` |
| `set_settings` | `settings: AppSettings` | `bool` |
| `get_ffmpeg_status` | — | `FfmpegStatus` |
| `update_ffmpeg` | — | `Channel<FfmpegProgress>` |

### Events (Backend → Frontend, push notifications)

| Event | Payload |
|-------|---------|
| `download:queued` | `{ task_id, position }` |
| `download:started` | `{ task_id }` |
| `download:muxing` | `{ task_id }` |
| `download:complete` | `{ task_id, file_path }` |
| `download:error` | `{ task_id, error }` |
| `download:paused` | `{ task_id }` |
| `download:resumed` | `{ task_id }` |
| `session:expired` | `{}` |
| `session:updated` | `{ username }` |
| `ffmpeg:status_changed` | `FfmpegStatus` |

### Channels (Backend → Frontend, streaming progress)

Each active download streams `DownloadProgress`:
```rust
pub struct DownloadProgress {
    pub task_id: String,
    pub phase: DownloadPhase,     // Downloading, Muxing, PostProcessing
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub eta_secs: Option<u32>,
}
```

### Supporting Types

```rust
pub enum QualityPref { Max, High, Medium } // Max = DASH mux, High = best direct URL, Medium = 720p

pub enum SessionStatus { Valid { username: String }, Expired, None }

pub enum FfmpegStatus {
    NotInstalled,
    Installed { version: String, path: PathBuf },
    Downloading { progress_pct: f32 },
    Updating { from: String, to: String },
    Error { message: String },
}

pub struct FfmpegProgress {
    pub phase: FfmpegPhase,          // Downloading, Extracting, Verifying
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub progress_pct: f32,
}

pub enum DownloadPhase { Downloading, Muxing, PostProcessing }

pub struct AppSettings {
    pub output_dir: PathBuf,         // Default: ~/Downloads/InstagramDownloaderPro/
    pub max_concurrent: u8,          // Default: 3
    pub request_interval_ms: u64,    // Default: 2000
    pub proxy: Option<ProxyConfig>,
    pub theme: Theme,                // Dark (default), Light
    pub organize_by_username: bool,  // Default: true
    pub auto_update_ffmpeg: bool,    // Default: true
}

pub struct ProxyConfig {
    pub proxy_type: ProxyType,       // Http, Socks5
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}
```

---

## 8. SQLite Schema

```sql
CREATE TABLE downloads (
    id TEXT PRIMARY KEY,
    shortcode TEXT NOT NULL,
    url TEXT NOT NULL,
    owner_username TEXT NOT NULL,
    media_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    quality_pref TEXT NOT NULL DEFAULT 'max',
    file_path TEXT,
    error_message TEXT,
    bytes_downloaded INTEGER DEFAULT 0,
    total_bytes INTEGER,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    retry_count INTEGER DEFAULT 0
);

CREATE TABLE history (
    id TEXT PRIMARY KEY,
    shortcode TEXT NOT NULL,
    url TEXT NOT NULL,
    owner_username TEXT NOT NULL,
    media_type TEXT NOT NULL,
    file_path TEXT NOT NULL,
    file_size INTEGER,
    downloaded_at INTEGER NOT NULL
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

---

## 9. Frontend Architecture

### State Management
- **Zustand** global store: auth status, queue state, settings, FFmpeg status
- Hydrated on app launch via `get_queue_state`, `check_session`, `get_settings`, `get_ffmpeg_status`

### Views
1. **Home / URL Input:** Paste bar + instant media preview card (thumbnail, caption, type, quality options)
2. **Queue:** Live download list with per-item progress bars, pause/resume/cancel controls
3. **History:** Completed downloads, grouped by username, open file/folder actions
4. **Settings:** Output folder picker, concurrent download limit, proxy config, theme toggle, FFmpeg status/update

### Custom Titlebar
Tauri `decorations: false` + custom React titlebar with drag region, minimize/maximize/close buttons matching Windows 11 style.

### GSAP Animations (`src/animations/`)
- `transitions.ts` — page route transitions (fade + slide)
- `progress.ts` — smooth progress bar interpolation (no jittery jumps)
- `queue.ts` — queue item enter (slide in), exit (fade out), reorder (FLIP)
- Centralized timelines, not scattered in component useEffects

### Shadcn/ui
Dark mode default. Components: Card, Button, Dialog, Toast, ScrollArea, Progress, Tabs, Input, Badge, DropdownMenu.

---

## 10. Utils Module

### `logger.rs`
- `tracing` subscriber with `tracing-appender` rolling file to `{app_data}/logs/`
- Log levels: DEBUG for dev, INFO for release
- Sensitive cookie values filtered from all output

### `proxy.rs`
- Optional SOCKS5/HTTP proxy support (user-configured)
- User-Agent rotation from a curated list of real browser UAs
- Exponential backoff retry: base 2s, max 60s, jitter
- Configurable request interval (default 2s between Instagram API calls)

### `fs.rs`
- Filename sanitization: strip illegal Windows chars, truncate to 200 chars
- Output path resolution: `{user_folder}/{username}/{shortcode}_{index}.{ext}`
- Temp file management for DASH video/audio streams before muxing
- Duplicate detection: skip if file already exists at target path

### `http.rs`
- Configured `reqwest::Client` with cookie jar, custom headers (`X-IG-App-ID`, etc.)
- Delegates retry logic to `proxy.rs`
- Connection pool and timeout configuration

### `error.rs`
- `thiserror`-based error hierarchy: `AppError`, `ExtractorError`, `DownloadError`, `FfmpegError`, `AuthError`, `DbError`
- All errors implement `serde::Serialize` for IPC transport

---

## 11. Security

| Concern | Mitigation |
|---------|------------|
| Cookie storage | OS keyring only (Windows Credential Manager) |
| FFmpeg execution | Absolute path, argument array, no shell, timeout watchdog |
| Input URLs | Regex validation before extraction |
| Filename injection | Sanitize via `fs.rs`, no user-controlled path components |
| Log leakage | tracing filter excludes session cookies |
| Rate limiting | Exponential backoff + configurable interval |
| Child process | stdin closed, stderr/stdout piped, exit code checked |

---

## 12. Phased Implementation Roadmap

### Phase 1: Core Engine & FFmpeg Manager
- Project scaffolding (Tauri v2 init, Vite + React)
- `utils/` module (error types, logger, http client, fs helpers)
- `ffmpeg/` module (manager, version, download, verify, extract)
- `extractor/` module (trait, types, DASH parser)
- `download/` module (muxer only — FFmpeg wrapper)
- Unit tests for FFmpeg manager, DASH parser, muxer

### Phase 2: Extraction & Auth
- `extractor/graphql.rs` (GraphQL API extractor)
- `extractor/page_parser.rs` (HTML fallback extractor)
- `auth/` module (WebView login, cookie extraction, keyring storage, session validation)
- `utils/proxy.rs` (backoff, UA rotation)
- Integration tests for extraction + auth flow

### Phase 3: Download Pipeline
- `db/` module (SQLite schema, migrations, models)
- `download/manager.rs` (queue orchestration, concurrency semaphore)
- `download/worker.rs` (per-download lifecycle, channel progress)
- `commands/` module (all Tauri IPC commands)
- Queue persistence and recovery tests

### Phase 4: Frontend
- Layout shell (custom titlebar, sidebar, routing)
- URL input + media preview
- Download queue view with live progress
- History view
- Settings panel
- Zustand store + IPC hooks
- GSAP animation system

### Phase 5: Polish & QA
- Error handling edge cases
- Rate limit resilience testing
- Memory profiling (large queues)
- Windows installer (Tauri bundler)
- End-to-end testing

---

## 13. Key Rust Crates

| Crate | Purpose |
|-------|---------|
| `tauri` v2 | App framework, IPC, WebView |
| `tokio` | Async runtime |
| `reqwest` | HTTP client |
| `serde` / `serde_json` | Serialization |
| `rusqlite` | SQLite |
| `keyring` | OS credential storage |
| `tracing` / `tracing-appender` | Structured logging |
| `thiserror` | Error types |
| `zip` | FFmpeg archive extraction |
| `sha2` | Checksum verification |
| `quick-xml` | DASH manifest parsing |
| `regex` | URL parsing, FFmpeg output parsing |
| `uuid` | Task IDs |
| `chrono` | Timestamps |
| `async-trait` | Trait async methods |

---

## 14. Verification Plan

1. **FFmpeg Manager:** Unit test — mock GitHub API response, verify download/extract/version logic. Integration test — actually fetch and verify ffmpeg.exe exists and runs `ffmpeg -version`.
2. **DASH Parser:** Unit test — parse sample MPD XML, assert correct highest-bitrate URL extraction.
3. **Muxer:** Integration test — mux a real video.mp4 + audio.m4a, verify output plays correctly.
4. **Extractors:** Integration test with a valid session — resolve a public post URL, verify MediaPost fields populated.
5. **Auth flow:** Manual test — open login window, authenticate, verify cookies stored in keyring.
6. **Download pipeline:** Integration test — enqueue a public post download, verify file saved to correct path with expected quality.
7. **Queue persistence:** Test — enqueue items, kill app, restart, verify queue recovered from SQLite.
8. **Frontend:** Manual test — paste URL, see preview, download, observe progress, check history.
