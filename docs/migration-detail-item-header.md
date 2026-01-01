# DetailItem::Header Migration Plan

**Date**: 2025-12-31
**Status**: Compilation errors identified
**Goal**: Remove deprecated `DetailItem::Header` variant, migrate to correct `ListItem::Header` architecture

## Background

The `DetailItem::Header` variant was deprecated in v0.11.0. Headers are **presentation-only** and belong in the UI layer as `ListItem::Header`, not in the domain layer.

**Correct architecture**:
```
┌─────────────────────────────────────────────────────────────────────┐
│ UI Layer (ListItem)                                                 │
│   - ListItem::Content(DetailItem) - actionable content              │
│   - ListItem::Header(String)      - section headers (non-focusable) │
│   - ListItem::Spacer              - visual spacing                  │
├─────────────────────────────────────────────────────────────────────┤
│ Domain Layer (DetailItem)                                           │
│   - DetailItem::Song(Song)        - playable content                │
│   - DetailItem::Ref(ContentRef)   - navigable reference             │
└─────────────────────────────────────────────────────────────────────┘
```

**Header representation** (during migration):
- Headers from API are Song objects with `metadata["type"] = "header"`
- `Song::is_header()` checks this metadata
- `DetailItem::is_header()` delegates to `Song::is_header()`

## Migration Points (9 errors)

### 1. Panes with Header Match Arms (6 files)

These files match on `DetailItem::Header { .. }` to skip action handling:

**Fix**: Replace `DetailItem::Header { .. }` with `item if item.is_header()` pattern

| File | Line | Context |
|------|------|---------|
| `src/ui/panes/queue_pane_v2.rs` | 190 | `handle_action()` match |
| `src/ui/panes/search_pane_v2.rs` | 435 | `handle_enter()` match |
| `src/ui/panes/search_pane_v2.rs` | 501 | `handle_action()` match |
| `src/ui/panes/album_detail.rs` | 123 | `handle_action()` match |
| `src/ui/panes/artist_detail.rs` | 126 | `handle_action()` match |
| `src/ui/panes/playlist_detail.rs` | 123 | `handle_action()` match |

### 2. Header Construction Sites (3 locations)

These files call `DetailItem::header()` to create headers:

| File | Line | Fix |
|------|------|-----|
| `src/ui/widgets/detail_stack.rs` | 134 | Error display - convert to Song or ListItem |
| `src/ui/widgets/detail_stack.rs` | 337 | Legacy header skip - check `is_header()` instead |
| `src/ui/widgets/detail_stack.rs` | 489 | Section title - convert to Song or ListItem |

## Migration Strategy

### Phase 1: Quick Fixes (Panes)
Replace match arms with `is_header()` checks:
```rust
// BEFORE
match item {
    DetailItem::Header { .. } => PaneAction::Handled,
    DetailItem::Song(s) => ...
}

// AFTER
match item {
    item if item.is_header() => PaneAction::Handled,
    DetailItem::Song(s) => ...
}
```

### Phase 2: detail_stack.rs Migration
This is the critical file that builds `Vec<DetailItem>` for display.

**Option A**: Keep using Song-based headers (simple migration)
```rust
// Line 134 - Error case
let mut metadata = HashMap::new();
metadata.insert("type".into(), vec!["header".into()]);
metadata.insert("title".into(), vec![format!("Error: {}", msg)]);
vec![DetailItem::Song(Song { metadata, ..Default::default() })]

// Line 489 - Section titles
let mut header_song = Song::default();
header_song.metadata.insert("type".into(), vec!["header".into()]);
header_song.metadata.insert("title".into(), vec![section.title.clone()]);
items.push(DetailItem::Song(header_song));
```

**Option B**: Convert to `Vec<ListItem>` (proper architecture)
- Change function signature to return `Vec<ListItem>` instead of `Vec<DetailItem>`
- Wrap actionable items: `ListItem::Content(DetailItem::Song(...))`
- Use proper headers: `ListItem::Header(section.title)`
- Update all callers to handle `ListItem` enum

### Phase 3: Test Verification
Run tests to confirm migration:
```bash
cargo test domain::detail_item
cargo test ui::widgets::detail_stack
cargo test ui::panes
```

## Implementation Order

1. ✅ Remove `DetailItem::Header` variant from enum
2. ✅ Update `ListItemDisplay` implementation
3. ✅ Update `From<Song>` conversion
4. ✅ Fix tests
5. ⏳ Fix panes (6 files) - simple match arm changes
6. ⏳ Fix detail_stack.rs (3 locations) - needs architectural decision
7. ⏳ Verify all tests pass
8. ⏳ Test in running application

## Decision Needed

**For detail_stack.rs migration**: Should we:
- **Option A**: Keep using Song-based headers (simpler, works now)
- **Option B**: Properly convert to `Vec<ListItem>` (correct architecture, larger change)

Recommend **Option A** for this migration, then create separate task for full ListItem conversion.

## Files Modified

- ✅ `src/domain/detail_item.rs` - Removed Header variant
- ⏳ 6 pane files - Need match arm updates
- ⏳ `src/ui/widgets/detail_stack.rs` - Need header construction updates
