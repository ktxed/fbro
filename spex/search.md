# Search Functionality

## Overview

Search is a full-featured file search capability that operates as a **job-based system** within the fbro architecture. It follows the BE-authoritative model where the backend owns all search logic, execution, and results storage, while the frontend renders the search modal, displays results, and sends intents via WebSocket.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Frontend (React/TypeScript)                                │
│  ┌──────────────┐    ┌──────────────┐    ┌───────────────┐  │
│  │ SearchModal  │    │ Store        │    │ ws/types.ts   │  │
│  │ (UI)         │◄──►│ (pending     │◄──►│ (TS types)    │  │
│  │              │    │  Search)     │    │               │  │
│  └──────────────┘    └──────────────┘    └───────────────┘  │
└───────────────────────────┬─────────────────────────────────┘
                            │ WebSocket
┌───────────────────────────┴─────────────────────────────────┐
│  Backend (Rust/Tauri)                                       │
│  ┌──────────────┐    ┌──────────────┐    ┌───────────────┐  │
│  │ Intent       │    │ Job System   │    │ VFS Driver    │  │
│  │ Handlers     │───►│ (search      │───►│ (search       │  │
│  │ (jobs.rs)    │    │  workers)    │    │  results)     │  │
│  └──────────────┘    └──────────────┘    └───────────────┘  │
│  ┌──────────────┐    ┌──────────────┐                       │
│  │ Protocol     │    │ State        │                       │
│  │ Schemas      │    │ (search      │                       │
│  │              │    │  stores)     │                       │
│  └──────────────┘    └──────────────┘                       │
└─────────────────────────────────────────────────────────────┘
```

## Search Flow End-to-End

1. **User triggers search** (F3 or command palette)
2. **FE opens SearchModal** with input tab
3. **User configures**: paths, depth, file mask, content query
4. **User clicks "Search" or "Background"**
5. **FE sends StartSearch intent** via WebSocket
6. **BE receives intent** and creates:
   - SearchParams struct
   - SearchResultStore (Arc<Mutex>)
   - Shadow source "search:<job_id>" with SearchResultsDriver
   - Search job with JobKind::Search
7. **BE sends JobStarted message** to FE
8. **BE spawns run_search worker** (8 concurrent workers)
9. **Workers traverse directories in parallel**:
   - Apply mask filter (substring/wildcard/regex)
   - Apply content filter (case-insensitive text search)
   - Resolve source_id for each result
10. **Results flushed in batches** (50 entries or 200ms interval)
11. **BE sends JobEvent with searchProgress** to FE
12. **FE accumulates results** in pendingSearch state
13. **User can**:
    - View results in modal's Results tab
    - Click "To Results" to mount in panel
    - Cancel search
    - Close modal (search continues in background)

## Search Features

### Depth Options

| Option | Description |
|--------|-------------|
| `unlimited` | Recurse all subdirectories |
| `current` | Only the specified directory (depth 0) |
| `depth N` | Recurse up to N levels deep (1-10 available in UI) |

### File Name Masking (3 modes)

| Mode | Description | Example |
|------|-------------|---------|
| `substring` | Case-insensitive substring match | `src` matches `src/`, `source.rs` |
| `wildcard` | Glob patterns (`*` → `.*`, `?` → `.`) | `*.txt` matches all text files |
| `regex` | Full regex matching, case-insensitive | `^test.*\.rs$` |

### Content Search

- Case-insensitive text search within file contents
- Chunked reading (64 KiB buffers) with boundary carry-over
- Only applied to files, not directories
- Performance-optimized with early exit on first match

### Multi-Path Search

- Search multiple directories simultaneously
- Add paths manually or select from mounted volumes
- Source resolution per result based on path matching

### Background Execution

- Search runs as a job that can continue after modal closes
- Results accessible via JobsBar
- Cancelable at any time

## Data Structures

### Backend (Rust)

#### Search Depth

```rust
pub enum SearchDepth {
    Unlimited,
    Current,
    Depth(u8),
}
```

#### Mask Mode

```rust
pub enum MaskMode {
    Substring,
    Wildcard,
    Regex,
}
```

#### Search Params

```rust
pub struct SearchParams {
    pub paths: Vec<String>,
    pub depth: SearchDepth,
    pub file_mask: Option<String>,
    pub mask_mode: MaskMode,
    pub content_query: Option<String>,  // Always case-insensitive
}
```

#### Search Result Entry

```rust
pub struct SearchResultEntry {
    pub name: String,
    pub kind: String,        // "file" | "directory"
    pub size: Option<u64>,
    pub parent_path: String,
    pub full_path: String,
    pub source_id: String,   // Real source that owns this entry
}
```

#### Search Result Store

```rust
pub struct SearchResultStore {
    pub results: Vec<SearchResultEntry>,
    pub done: bool,
}
```

### Protocol Schemas

#### SearchParamsSchema

```rust
pub struct SearchParamsSchema {
    pub paths: Vec<String>,
    pub depth: String,           // "unlimited", "current", or number
    pub file_mask: Option<String>,
    pub mask_mode: String,       // "substring", "wildcard", "regex"
    pub content_query: Option<String>,
}
```

#### SearchResultSchema

```rust
pub struct SearchResultSchema {
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    pub parent_path: String,
    pub full_path: String,
    pub source_id: String,
}
```

#### SearchProgressSchema

```rust
pub struct SearchProgressSchema {
    pub scanned: u64,
    pub matched: u64,
    pub current_path: String,
    pub estimated_percent: u8,
    pub new_results: Vec<SearchResultSchema>,
}
```

### Frontend (TypeScript)

#### SearchParams

```typescript
interface SearchParams {
  paths: string[];
  depth: string;
  fileMask?: string | null;
  maskMode: string;
  contentQuery?: string | null;
}
```

#### SearchResult

```typescript
interface SearchResult {
  name: string;
  kind: string;
  size?: number;
  parentPath: string;
  fullPath: string;
  sourceId: string;
}
```

#### SearchProgress

```typescript
interface SearchProgress {
  scanned: number;
  matched: number;
  currentPath: string;
  estimatedPercent?: number;
  newResults: SearchResult[];
}
```

#### PendingSearch

```typescript
interface PendingSearch {
  sourcePanelPath: string;
  jobId: string | null;
  searchSourceId: string | null;
  results: SearchResult[];
  scanned: number;
  matched: number;
  currentPath: string;
  estimatedPercent?: number;
  status: "idle" | "running" | "completed" | "cancelled" | "failed";
  activeTab: "input" | "results" | "extra";
  requestId: string | null;
  searchParams: SearchParams | null;
}
```

## UI Components

### SearchModal

**Three tabs**: "Search Input" (mnemonic: s), "Results" (mnemonic: r),
and "Extra" (mnemonic: e, size and datetime filters).

#### Input tab sections

1. **Search Locations**: path input, volume selector, add/remove paths, depth selector
2. **File Name**: mask input with mode radio buttons (substring/wildcard/regex)
3. **Content**: textarea for text search query

#### Results tab

- A persistent activity strip below the tabs remains visible on Input, Results,
  and Extra while running: estimated progress bar, matched count, inspected-entry
  count, and current folder. Activity updates never move focus.
- Completed searches show final matched and inspected-entry counts, including
  searches with zero results; cancellation and failure are identified explicitly.
- Result list (capped at 500 displayed, with warning if more)
- Each result shows icon, name, parent path, copy path button
- Double-click navigates to the result's location

#### Action buttons

- **Search**: Run search and stay in modal
- **Background**: Run search and close modal (continues in background)
- **Cancel Search**: Stop the running search
- **To Results**: Mount search results in panel
- **Close**: Close modal

### Integration Points

- **JobsBar**: Search chips show the same backend estimate and current folder,
  plus the hit count. Long folders truncate with a full-path tooltip. Clicking
  or keyboard activation reopens SearchModal with cached activity and results.
  Completion, cancellation, and failure stop animation.
- **Layout.tsx**: Renders SearchModal when `state.pendingSearch` is not null
- **CommandPalette**: "panel.filter.open" command opens filter mode (related but separate from search)

## Search Results as Virtual Filesystem

The `SearchResultsDriver` implements `VfsDriver` to present search results as a flat filesystem:

- Root path is always `/`
- Lists all results as flat entries (no hierarchy)
- `parent_path()` returns `None` (navigating up exits the search source)
- Read/write/delete/move operations return `NotSupported` errors
- `resolve_child()` returns the actual full_path for navigation

### Mounting Search Results

When user clicks "To Results":

1. BE creates shadow source `search:<job_id>`
2. Panel's `source_id` switches to search source
3. Panel's `path` set to `/`
4. Panel's quick filter and filter-entry mode are cleared
5. Panel rescans using `SearchResultsDriver.list()`
6. Results appear as a flat panel listing

## Navigation from Search Results

### Viewer files

Opening the viewer resolves the backend panel's cursor entry through its
`source_path` and looks up the exact result's owning source. Viewer j/k
navigation uses the destination row's `source_path` in the same way. Duplicate
filenames never identify a viewer target, and panel indices must not be used
as indices into the search store (arrival order can differ from panel sorting).
An unavailable result reports an error rather than opening another same-name
file.

### NavigateToSearchResult Intent

- For directories: navigates to the directory's real path
- For files: navigates to parent directory with cursor on the file
- Tears down the search shadow source before navigating
- Resolves the best real source for the target path

### RemoveSearchSource Intent

- Cancels running search job
- Finds panels using the search source
- Navigates them back to parent source/path
- Removes source, driver, and store from state

## Performance Characteristics

| Constant | Value |
|----------|-------|
| `MAX_CONCURRENT` | 8 workers |
| `FLUSH_BATCH` | 50 results |
| `FLUSH_INTERVAL` | 200ms |
| `CONTENT_READ_BUF` | 64 KiB |

- **Worker pool**: 8 concurrent workers
- **Work queue**: An unbounded `async-channel` directory frontier. Enqueuing
  children never waits for a consumer: the same eight workers produce and
  consume directories, so a bounded frontier can deadlock with all workers
  waiting to enqueue. Active directory reads remain limited to eight and the
  independent result collector retains its bounded 1024-entry channel.
- **Result channel**: 1024 capacity
- **Flush batching**: 50 results or 200ms interval. The 200ms timer emits activity
  even when no results arrive, including during a large content read.
- **Activity accounting**: `scanned` counts directory entries inspected before
  metadata and search filters, rather than matches. `currentPath` is the most
  recently started still-active worker folder, or empty when none is active.
- **Estimate**: `estimatedPercent` is completed folders / discovered folders,
  capped at 99 while running. It may decrease as more folders are discovered;
  no preliminary traversal is performed. Only successful completion reports 100.
  The bar continues to animate during unchanged estimates; reduced-motion
  preferences disable animation. Older payloads without the estimate use an
  indeterminate bar.
- **Cancellation**: signal workers and drain their result channel rather than
  aborting the collector. Content readers check cancellation between chunks.
  Final counts and buffered results arrive before the terminal event; terminal
  `searchProgress` has no new results and retains the estimate for cancellation.
  Both successful and cancelled terminal events clear the current folder.
- **Access failures**: attempt each directory listing once; unreadable directories
  and entries whose metadata cannot be read are skipped without retrying or
  stopping accessible sibling traversal. Directory symlinks are not followed.
- **Content reading**: only regular files, in 64 KiB chunks with boundary
  carry-over. Pipes, sockets, devices, and symlinks are skipped when a content
  filter is active; searching their names remains supported.
- **UI cap**: First 500 results shown in modal, full list available in panel

## Code Locations

### Backend (Rust)

| File | Purpose |
|------|---------|
| `src-tauri/src/jobs/search.rs` | Core search engine - worker pool, directory traversal, mask matching, content search |
| `src-tauri/src/vfs/search.rs` | Virtual filesystem driver that presents search results as a flat filesystem |
| `src-tauri/src/ws/intents/jobs.rs` | Intent handlers: `start_search`, `navigate_to_search_result`, `mount_search_results`, `remove_search_source` |
| `src-tauri/src/ws/protocol.rs` | Protocol schemas: `SearchParamsSchema`, `SearchResultSchema`, `SearchProgressSchema` |
| `src-tauri/src/ws/intents/mod.rs` | Intent dispatch routing for search intents |
| `src-tauri/src/jobs/mod.rs` | `JobKind::Search` enum variant and `SearchJobMeta` struct |
| `src-tauri/src/state/mod.rs` | `search_stores` field in AppState |
| `src-tauri/src/ws/server.rs` | Search job event handling and store completion marking |
| `src-tauri/src/ws/intents/viewer.rs` | Viewer intent handling for search result sources |
| `src-tauri/src/ws/intents/navigation.rs` | Navigation handling when on search result sources |

### Frontend (TypeScript/React)

| File | Purpose |
|------|---------|
| `src/components/SearchModal.tsx` | Main search modal component with input/results tabs |
| `src/store/index.ts` | `PendingSearch` interface, reducer actions, BE message handling |
| `src/ws/types.ts` | TypeScript types: `SearchParams`, `SearchResult`, `SearchProgress`, Job search fields |
| `src/styles.css` | Search modal CSS styling |

## Search vs Filter Distinction

The codebase has two separate filtering concepts:

1. **Panel Filter** (`SetFilter` intent): Filters directory listings by filename prefix/pattern. Applied during directory scan, BE-owned.

2. **Search** (`StartSearch` intent): Full recursive file search across multiple paths with name and content matching. Runs as a job, results presented as virtual filesystem.

## WebSocket Protocol

### StartSearch Intent (FE → BE)

```json
{
  "type": "Intent",
  "name": "StartSearch",
  "args": {
    "requestId": "uuid",
    "paths": ["/home/user"],
    "depth": "unlimited",
    "fileMask": "*.txt",
    "maskMode": "wildcard",
    "contentQuery": "hello world",
    "background": false
  }
}
```

### JobStarted Message (BE → FE)

```json
{
  "type": "JobStarted",
  "jobId": "job-1",
  "kind": "search"
}
```

### JobEvent with SearchProgress (BE → FE)

```json
{
  "type": "JobEvent",
  "jobId": "job-1",
  "event": "Progress",
  "searchProgress": {
    "scanned": 1500,
    "matched": 42,
    "currentPath": "/home/user/docs",
    "estimatedPercent": 37,
    "newResults": [
      {
        "name": "notes.txt",
        "kind": "file",
        "size": 1024,
        "parentPath": "/home/user/docs",
        "fullPath": "/home/user/docs/notes.txt",
        "sourceId": "src-1"
      }
    ]
  }
}
```
