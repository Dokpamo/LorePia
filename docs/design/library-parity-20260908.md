# Home and Chats control parity — 2026-09-08

The later [search page update](./library-search-page-20260908.md) moves the search
field into a dedicated page. The control sizes and spacing in this record are
the baseline for that update.

Task ID: UI-LIBRARY-PARITY-20260908. Baseline: `2200173`; the existing four-tab
redesign and library work are uncommitted and must be preserved. The architecture
baseline passes (`/tmp/lorepia-library-parity-baseline.txt`).

This is a requested presentation update. Home adopts the existing Chats search
height of 44px. Both libraries use the same search, chip and sort geometry:
header → search → category chips → left-aligned sort → results. Home tags remain
an explicit layout mockup; Chats character filters operate on real conversations.

Targets: `ui/navigation/` library components/styles/view types, the conversation
projection in `WorkspaceApp.svelte`, navigation locale keys and the four-tab
integration suite. Shared search/clear and sort controls move into bounded
components; the CharacterLibraryFilters entry and CharacterLibrarySort type stay
available. No native, IPC, dependency or storage changes. Expected net delta:
roughly 150–250 lines including checks and this record, with no baseline increase.

Owned invariants: per-tab queries and filters survive navigation; clearing search
restores focus; sort uses saved timestamps with deterministic tie-breaks and
does not mutate the controller's catalog; opening a chat, drafts, settings and
choice-sheet focus behavior remain under their existing owners. Main risk:
filtering and sorting must compose without dropping the saved search or draft.

Verification: existing WorkspaceFourTabs tests before the change; extend its
interaction coverage for combined chat filtering, ordering and clearing; frontend
checks, architecture, browser geometry at 320/390/430/desktop widths, native build
and direct inspection.

Sources reviewed:
- https://seed-design.io/foundations/spacing — shared 8/12/16px spacing scale.
- https://seed-design.io/components/chip — horizontal chips and consistent sizes.
- https://seed-design.io/components/list — prefix/title/detail/suffix alignment.
- https://seed-design.io/components/text-input — clear action and input anatomy.

The 44px filled search height follows the user's existing Chats preference; it is
not presented as the specification for SEED's 52px large form field.

## Result and measurement

Both roots now render the same LibrarySearch and LibrarySort components. The
44px field has a square 20px Lucide search icon and 12px horizontal padding;
the clear action has a 44px target and restores input focus. Chats uses real
character chips instead of the earlier wrapping gray dropdown. Its sort uses
the existing `updated_at` field and supports newest, oldest and title order.

At 390px wide, both search surfaces are x24/y72/w342/h44. Visible chips start
at y132 and are 36px high, inside 44px targets. Sort rows start at y184 and are
44px high; the results area starts at y236. The same search/chip/sort geometry
matches between roots at 320, 430 and 1280px too. Vertical gaps remain 12px from
header to field, 16px from field to visible chip, 16px from visible chip to sort
target and 8px from sort target to results. Conversation rows additionally have
12px internal vertical padding; their 48px avatars and title/detail block use
a 12px horizontal gap. List gear centers align with the header action center
at all four measured widths, including the bounded desktop layout.

The browser inspection covers horizontal overflow, search clearing/focus,
filtering, sort choices, empty results, settled dark appearance and 125% text at
320px. The native app was rebuilt and its actual Home and Chats screens inspected.
Screenshot and geometry evidence is in `/tmp/lorepia-library-layout/`; the
pre-change integration suite passed, and the updated suite also covers composing
chat filters with sorting/search and preserving them across tab switches.
