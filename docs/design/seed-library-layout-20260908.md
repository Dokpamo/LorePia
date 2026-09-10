# Home discovery layout — 2026-09-08

The latest [search page update](./library-search-page-20260908.md) replaces the
root search field with a header action. The measurements below document the
preceding inline-search layout.

The agreed order is title/actions → search → tag chips → sorting → character
cards. Tags are explicitly a **layout mockup** for now: selecting an example
changes its selected appearance without filtering or writing saved character
data. Search and date/name sorting operate on the real library.

## Sources and application

- [SEED Spacing](https://seed-design.io/foundations/spacing) provides 8px between
  chips and a 12px default gap between related components. Those tokens group
  the search/filter controls; an 8px gap connects the sort row to its results.
- [SEED Chip](https://seed-design.io/components/chip) defines a 36px medium chip,
  full corner radius, and scrollable chip groups. The examples use the quieter
  outline selection treatment, a 14px label, and a separate 44px touch target.
- [SEED Text Input](https://seed-design.io/components/text-input) informs the
  prefix/value/clear anatomy. The user subsequently chose the existing Chats
  search's compact 44px height for both libraries, with a 20px icon and 16px text.
  This is a filled search control, not SEED's 52px large form field.
- [SEED Layout](https://seed-design.io/foundations/layout) supplies fluid content
  and bounded desktop width. The current 1040px content container is retained.
  Its project-specific outer gutter follows the already agreed header layout;
  search, chips, sort and cards share that edge.

## Measured geometry

At a 390 × 808 CSS-pixel viewport:

| Element | X | Y | Width | Height |
| --- | ---: | ---: | ---: | ---: |
| Header | 0 | 0 | 390 | 60 |
| Search surface | 24 | 72 | 342 | 44 |
| First chip, visible surface | 24 | 132 | 50.23 | 36 |
| First chip, touch area | 24 | 128 | 50.23 | 44 |
| Sorting row | 24 | 184 | 342 | 44 |
| Card grid start | 24 | 236 | 342 | — |

The header-to-search gap is 12px. The search-to-visible-chip gap is 16px,
including 4px inside the chip's touch area. Chips are separated by 8px; the
sort row ends 8px before the card grid. Keyboard focus has a 2px inset allowance
around the horizontal chip rail, which does not change the visible chip edge.

The sorting label is left-aligned with the search surface, first chip and cards
(x24 at 390px wide). Its touch target extends 8px to the left to retain the
button's internal padding. A recheck at 320, 390 and 430px confirmed matching
left edges and unchanged vertical gaps of 12px, 16px, 16px and 8px, measured
between the header, search surface, visible chips, sorting row and card grid.

At 320px wide the rail scrolls horizontally and the cards start at y228;
at 430px they start at y242.14. No viewport overflow was observed at 320, 390,
430 or 1280px. Sorting uses `created_at` already supplied by Rust; ties fall
back to character name and ID without mutating the source array. No Rust DTO,
schema or command was changed for this layout.

The integration check covers newest/oldest ordering, search clearing/focus,
and the explicit tag-mockup behavior. The browser run also exercised name sorting
and reported no page errors.

For the later shared Home/Chats controls and validation, see
[the control parity record](./library-parity-20260908.md).
