# Navigation pixel comparison — 2026-09-08

Reference: `/Users/codexer/Downloads/photo_6125363692363780866_y.jpg`,
618 × 1280 pixels. The user confirmed that LorePia's title and upper buttons
belong on the **same row**. A follow-up asks for smaller titles on every root,
no redundant introduction/count, and a filled selected tab icon instead of a
selection capsule. These instructions supersede the first title/selection pass.

## Method

The image is normalized uniformly to 390px wide (factor 390/618), giving a height
of 807.77px. Browser captures use 390 × 808 CSS pixels at device scale factor 1.
The 0.23px height rounding is excluded by comparing the bar's bottom inset.

Pixel analysis of the JPEG identifies the main white capsule component at
(21, 1163), 576 × 97px. Dark-pixel bounds use RGB channels below 150; capsule
pixels use channels at least 251. JPEG antialiasing makes edges approximate to
one source pixel, so subpixel DOM precision is not a claim of exact raster identity.

## Comparison at 390px

| Measurement | Reference, normalized | Implemented | Difference |
| --- | ---: | ---: | ---: |
| Capsule left inset | 13.25px | 13.25px | < 0.02px |
| Capsule width | 363.50px | 363.48px | < 0.02px |
| Capsule height | 61.21px | 61.20px | < 0.02px |
| Capsule bottom inset | 12.62px | 12.61px | < 0.02px |
| Title visible left edge | 27.14px | 27px | 0.14px |
| Title visible height | 25.24px | 22px | Intentionally smaller |
| Trailing action visible center X | 350.87px | 351px | 0.13px |
| Trailing action visible center Y | 30.61px | 30px | 0.61px |

The first pass matched the reference's title ink at 28px, but that made the
same-row root titles too large beside their actions. All four root titles now
use 24/32 at 390px and 20/26.67 at 320px. The app's `홈` title is measured from
the actual PNG. Font-size and visible glyph height are kept distinct. Upper
controls retain a minimum 44 × 44px touch area around their SVG artwork.

## Interior comparison and correction

The earlier capsule measurement did not catch a shared `svg` flex-basis of
24px. It forced bottom icons to stay 24px tall even when their CSS width grew.
The bottom SVG flex-basis now follows the icon slot. Its height, label size,
top inset and vertical gap scale with the capsule's actual container width.
Lucide artwork is optically sized inside that slot, including a smaller House
silhouette so its filled state has a similar visual weight to the reference.

| Interior measurement at 390px | Reference, normalized | Final rendered result |
| --- | --- | --- |
| Selected silhouette | 21.46 × 20.19px | House: 20 × 21px |
| Unselected artwork | 23.98–25.87 × 21.46–23.35px | 22–24 × 23–24px |
| Label visible height | 10.10px | 10–11px |
| Label visible top | 774.32px | 774–775px |
| Right upper action center | (350.87, 30.61) | (351, 30) |
| Upper back target center | — | (28, 30), 44 × 44px target |

These are comparisons of occupied pixels, not claims that different icon paths
or Korean characters are identical. The reference has its own stock/heart/planet/
feed symbols; LorePia retains Lucide Home/Chat/Create/Settings. The selected
icon uses a near-black fill in light mode and a light fill in dark mode. The
selection pill is removed; press scaling and keyboard focus remain available.

Home's repeated heading/count, Create's introduction, and the chat count are
removed. Search, character names, useful section labels and filtering remain.

LorePia retains four root destinations. The reference's additional host-back
control is not a fifth destination in this standalone app; the four cells share
the capsule evenly. All icons remain Lucide with the previously selected stroke.

## Responsive and interaction checks

| Viewport width | Capsule width × height | Tab target height | Label font size |
| --- | --- | --- | --- |
| 320px | 298.25 × 50.22px | 50.22px | 10px |
| 390px | 363.48 × 61.20px | 61.20px | 11.99px |
| 430px | 400.77 × 67.48px | 67.48px | 13.22px |
| 474px | 440.00 × 74.09px | 74.09px | 14.51px |
| 1280px | 440.00 × 74.09px | 74.09px | 14.51px |

All four roots share the same header and bar measurements at 390px. No horizontal
overflow was observed. The header action slot, gutter and header height also
grow through the mobile width range, with bounded desktop sizes. Root title
sizing is bounded for small/large viewports; the reference comparison is anchored
at 390px, not a claim that text scales indefinitely with desktop window width.

The frontend check, seven existing settings/navigation integration tests,
portable runtime checks and prescribed architecture check passed. Existing
ChatPane initial-controller-reference warnings are unchanged. Tauri was rebuilt
and its Home/Create screens inspected directly. A fresh preview profile also
passed the dark-theme selection check: a light icon fill, transparent selection
background, and the same icon/label geometry.

## Design basis

- [SEED Top Navigation](https://seed-design.io/components/top-navigation): distinct
  root/detail roles, stable action slots and a separate touch target from artwork.
- [SEED Typography](https://seed-design.io/foundations/typography): named size,
  line-height and weight roles. Root titles now use 24/32; section headings
  remain 18/24 and body text 16/22.
- [SEED Bottom Navigation](https://seed-design.io/components/bottom-navigation):
  concise icon/label pairs and bounded container width. SEED recommends filled
  icons for both states; the user's specific Toss reference and request instead
  govern the outlined-unselected/filled-selected treatment here.
- [Lucide](https://lucide.dev/): existing packaged icon paths, with size/fill
  presentation customized in CSS; no replacement icon library or traced paths.

The financial screen's content, branding, colors and extra actions are not copied.
