# Memory & Design Audit — Remaining Items

Fixed items removed. See git history for the original audit.

---

## 1. Layout rendering heuristics (not exposed, low priority)

Internal rendering constants — not memory limits, don't cause data loss. Candidates for future `LayoutConfig` fields if callers need control:

| Constant | Value | Location |
|---|---|---|
| Image minimum width/height | 24.0/18.0 px | `render_layout.rs` |
| Full-width image min width | 60.0 px | `render_layout.rs` |
| Full-width image height multiplier | 1.35x, min 36.0 px | `render_layout.rs` |
| Cover heading font scale | 0.92, floor 12.0 px | `render_layout.rs` |
| Cover heading line-height | 1.3 | `render_layout.rs` |
| Justification stretch ratio | 0.0–1.2 | `render_layout.rs` |
| Badness penalty | 1000 | `render_layout.rs` |
| Widow/orphan clamp | 0.0–8.0 | `render_layout.rs` |
| Monospace width estimates | 0.52–0.58 em | `render_layout.rs` |
| Font size threshold | 24.0 px | `render_layout.rs` |

---

## 2. Unimplemented feature flag

| Field | Struct | Status |
|---|---|---|
| `enabled` | `HangingPunctuationConfig` | Config exists, no implementation |

---

## 3. Data model issues

### 3a. `ResolvedTextStyle.family: String` — most-cloned allocation

Cloned ~40+ times per page. Only 3–5 unique family names per book. Should be interned (u8 ID or `Arc<str>`).

### 3b. `StyledRun.resolved_family` — redundant field

Derivable from `style.family_stack` + font resolver. 24 extra bytes per run.

### 3c. `ComputedTextStyle.family_stack: Vec<String>` — oversized

Typical use is 1–2 entries. `SmallVec<[String; 2]>` or similar would avoid heap vec.

### 3d. `RenderPage.commands` — triple command storage on non-espidf

Four command vecs, `sync_commands()` clones three into the fourth. Roughly doubles command memory per page.

### 3e. `PageAnnotation.kind: String` — stringly-typed enum

Always from a small known set. Should be an enum.

### 3f. `ParagraphWord` — stores full style per word

Up to 64 words buffered, each with a full `ResolvedTextStyle` clone. Most words share the same style. Style-run encoding would deduplicate.

### 3g. Navigation `NavPoint.children` — recursive unbounded nesting

No depth limit. A flat `Vec<(depth, NavPoint)>` would eliminate recursive allocations.

---

## 4. Hidden allocations in hot paths

### 4a. Per-word `style.clone()` in layout main loop

5000 `ResolvedTextStyle` clones per chapter, each heap-allocating `family: String`. Blocked on 3a (interning).

### 4b. `Vec<char>` per hyphenation candidate

Could use `char_indices()` instead of collecting to `Vec<char>`.

### 4c. Two String allocs per candidate in `optimize_overflow_break`

`.join()` inside a loop creates ~18 Strings per overflow event. Could measure with slices.

### 4d. `concat()` per iteration in soft-hyphen loop

`prefix` and `suffix` allocate fresh each iteration. `suffix` computed even when prefix already too wide.

### 4e. Intermediate `Vec<&str>` in `layout.rs::text()`

Allocates a `Vec<&str>` just to call `.join()`. Could fold directly into a String.

### 4f. Font face clone during resolution

Font face objects cloned when collecting candidates. Warm path.

---

## 5. `sync_commands()` called eagerly — O(n²)

Called after every command push, re-clones all commands each time. For a 50-command page, that's ~1250 clone operations total over the page's lifetime. Should defer to a single sync at page finalization.
