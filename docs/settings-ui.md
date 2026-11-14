# Settings UI

The native settings window uses the existing egui viewports, theme palette and
`Settings` object. Changes still apply immediately and use the existing save
path. Search, pending shortcut confirmations and navigation are transient UI
state; no configuration keys or defaults were changed.

`src/app/settings_ui.rs` contains the small shared helpers:

- `Setting` defines searchable metadata and a stable identity for each canonical
  row. Add metadata here and use the same identity at the control with `row` or
  `text`. Search indexes categories, sections, labels, descriptions and aliases.
- `group` provides section hierarchy and spacing; `row` scopes widget state and
  handles search navigation and the temporary outline.
- `code` displays selectable, wrapping paths or commands with a copy action.
- `State` manages queries, category restoration and pending shortcut conflicts.

The sidebar and page title stay outside the content scroll area. Global search
is anchored at the bottom of the sidebar. A clear button appears inside the
right edge of the field only while the query is nonempty.
Each category has its own scroll state. Search results navigate to existing
controls rather than rendering duplicate editors. Text field results receive
keyboard focus; Auto Stitch advanced results also expand their disclosure.
Shared stitch controls only consume navigation inside the settings UI scope.

Ctrl+F focuses global search. Esc clears search and restores the previous page;
with no query, it cancels a pending confirmation or closes settings. During
shortcut recording, Esc cancels recording instead. Shortcut filtering is a
separate field on the Shortcuts page. Conflicting assignments and individual
resets wait for an explicit move confirmation; resetting all bindings requires
confirmation too.

## Verification

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`. Local socket
permissions are needed for the MCP regression tests. Optional media/tool tests
retain their existing ignored status.

The settings tests cover aliases, category matching, navigation and category
restoration, actual text field focus and scrolling, isolation from shared main
window controls, shortcut conflicts, concise tool versions, and wrapping long
commands in every theme.

For visual checks, use an isolated application data directory so changes do not
affect personal settings. Check all categories at 900 × 620 and 700 × 480 logical
pixels, every theme, and 100%, 125%, 150% and 200% interface scale. Check search
navigation to hardware decoding and advanced stitching, Esc restoration,
shortcut recording, and selectable/copyable paths and commands.
