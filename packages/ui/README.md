# Shared UI package

`@aidex/ui` holds the design tokens and accessible React components that AiDEX frontends are built from. App screens compose these components; they do not define their own styled markup. Add a component here, rather than in an app, whenever a screen needs a new visual building block.

## Entry points

| Import | Contents |
| --- | --- |
| `@aidex/ui/styles.css` | Design tokens, base element styles, the light theme, density blocks (`html[data-ui-density]`, legacy `html[data-density="compact"]`), the `data-chat-width` / `data-message-spacing` blocks, thin 6px scrollbars, and `html, body { overflow: hidden }`. Import once in the app root layout. |
| `@aidex/ui/themes` | `accentPresets`, `DEFAULT_ACCENT`, and `applyAccent(color)` for runtime accent changes; `applyAppearance(tokens, root?)`, `appearanceAttributes`, `appearanceVariables`, `codeLineHeight`, `DEFAULT_APPEARANCE_TOKENS`, the `*_RANGE` constants, and the `AppearanceTokens` / `UiDensity` / `ChatWidth` / `MessageSpacing` types for the sizing and layout options. |
| `@aidex/ui/icons` | Project-owned 24×24 stroke icons, the AiDEX `LogoMark`, and `BrandIcon` provider logos. |
| `@aidex/ui/primitives` | Controls, form fields, display atoms, and layout. |
| `@aidex/ui/components` | Product components, data display, and content display. |
| `@aidex/ui/desktop` | Desktop frames: shell, navigation, page layouts. |
| `@aidex/ui/installer` | Installer wizard frame. |

The package ships TypeScript source and CSS Modules; consuming Next.js apps list it in `transpilePackages`. Styles reference the `--aidex-*` custom properties in `src/tokens/tokens.css` instead of literal values. The accent family (border, soft, ring, focus) derives from `--aidex-color-accent`, so `applyAccent` re-themes all of it. Sizes are px tokens at the comfortable density (`--aidex-control-*`, `--aidex-row-*`, `--aidex-icon-*`, spacing, radius, shell and page widths, the `--aidex-z-*` ladder); `src/tokens/README.md` lists them with the density tables and the breakpoint constants. Keep body leading unitless (`--aidex-leading-body`).

Dialogs, menus, tooltips, selects, toasts, scroll areas and collapsibles are built on [Base UI](https://base-ui.com) (`@base-ui/react`, pinned to 1.4.1 and imported by subpath, e.g. `@base-ui/react/menu`) and wrapped in CSS-module components with the APIs below; apps never import Base UI themselves. See [Overlays and layers](#overlays-and-layers).

## Catalog

Every component accepts its root element's props (including `ref` and `className`) unless noted.

### Primitives

| Component | Key props |
| --- | --- |
| `Button` | `variant` primary · tonal · raised · outline · ghost; `size` md · lg · xl; `shape` rounded · pill; `align` start · center; `icon`, `trailing`, `fullWidth` |
| `IconButton` | `label` (required: the accessible name and the default tooltip), `icon`, `variant` ghost · soft · solid · primary, `size` sm · md · lg · xl, `shape`, `selected`, `tooltip` (content, or `false` for none), `tooltipSide`, `tooltipSideOffset` (4), `shortcut` (a `Kbd` in the tooltip). A `title` becomes the tooltip (e.g. a disabled reason) and is never rendered as a native `title`; text other than the label is also linked as the description. No tooltip while `disabled` (`aria-disabled` keeps it). Refs and other props reach the `<button>`, so it can be a `MenuTrigger` `render` or a popover anchor |
| `TextField` | `label`, `labelVisibility` visible · hidden, `hint`, `error`, `leadingIcon`, `trailing`, `shape` rounded · pill, `size` md · lg |
| `SearchField` | `TextField` with a search icon, pill shape, and hidden label |
| `PasswordField` | `TextField` with a reveal toggle (`revealLabel`); spell check and autocorrect are always off |
| `TextArea` | `label`, `labelVisibility`, `hint`, `error`, `monospace` |
| `Select` | `label`, `labelVisibility`, `options: { value, label, disabled? }[]`, `variant` field (32px) · ghost (28px), `hint`, `value` / `defaultValue`, `onChange`, `onValueChange(value)`, `name`, `required`, `disabled`, `id`, ARIA props. Base UI Select with the native select's API: `onChange` receives `{ target: { value, name }, currentTarget }`, `id` names the trigger (so `<label for>` and a `SettingRow` `controlId` reach it; a label press focuses it without opening), `ref` gives `focus()`, `blur()`, `value` and `name`, and `name` / `required` take part in forms through a hidden input. The list lines the chosen option up with the trigger; arrows, Home/End, typeahead and Enter work |
| `Checkbox` | `label`, `description` |
| `CheckboxGroup` | `label`, `options: { value, title, description?, meta?, disabled? }[]`, `value: string[]`, `onValueChange` (ruled rows) |
| `Switch` | `checked`, `onCheckedChange`, `label` (when no visible `<label htmlFor>`) |
| `SegmentedControl` | `label`, `options`, `value`, `onValueChange`, `size` sm · md (radio-group semantics) |
| `Tabs` + `TabPanel` | `id`, `label`, `items: { value, label, icon?, count? }[]`, `value`, `onValueChange`, `variant` pill · underline; wrap the selected content in `<TabPanel tabsId={id} value={value}>` |
| `ChoiceCardGroup` | `label`, `options: { value, title, description?, icon?, meta?, disabled? }[]`, `value`, `onValueChange`, `columns` 1 · 2 · 3, `variant` cards · rows |
| `SwatchPicker` | `label`, `swatches: { value, label }[]`, `value`, `onValueChange`, `allowCustom` |
| `Badge` | `tone` accent · success · warning · danger · neutral, `size` sm · md, `icon` |
| `StatusIndicator` | `label`, `tone` success · warning · danger · neutral (dot plus text) |
| `StatusDot` | `tone` success · warning · danger · neutral · accent, `size` sm (6px) · md (9px), `label` (announced as an image; without it the dot is decorative and nearby text must name the status) |
| `Popover` | `open`, `onOpenChange`, `anchorRef`, `placement` bottom-start · bottom-end · top-start · top-end (flips, and clamps 8px inside the window), `offset` (4), `label` or `labelledBy`, `role` dialog · menu · listbox · group, `initialFocusRef`, `autoFocus` (default true; false keeps focus in a text field that drives the popup), `matchAnchorWidth`, `container` (portal target; defaults to the enclosing `Dialog`, else `<body>`), `id`. Non-modal: focus moves in on open; Escape closes and returns focus to the anchor; a press outside or focus leaving closes it; Tab past either end leaves as if the popover sat right after its anchor, and closes it. It follows the anchor through layout shifts, hides while the anchor is scrolled away (`data-anchor-hidden`), exposes `--aidex-popover-available-width` / `-available-height` / `-anchor-width`, and fades out over 150ms (at once under reduced motion). Not a root-element passthrough |
| `Tooltip` + `TooltipProvider` | `Tooltip`: `content`, `children` (one trigger element), `side` top · bottom · left · right · inline-start · inline-end, `sideOffset` (4), `align`, `shortcut`, `disabled`, `delay`, `describe` (links the text as the trigger's description; on by default only when it differs from the trigger's name). Shows on hover and on keyboard focus the user moved there (Tab, arrow keys), not on focus a script placed (a dialog's first focus, focus going back to an opener), so a keyboard-opened dialog still closes with one Escape; hides on a press, Escape, or a scroll around the trigger. `TooltipProvider`: `delay` (600), `closeDelay` (0), `timeout` (400: the next tooltip opens at once). Not a root-element passthrough |
| `Menu` | `Menu` (root: `open`, `onOpenChange`, …), `MenuTrigger` (`render` an `IconButton` or `Button`), `MenuPopup` (`side` bottom, `align` start, `sideOffset` 4, `anchor`), `MenuItem` (`icon`, `shortcut`, `tone` default · danger, `disabled`, `onClick`), `MenuCheckboxItem`, `MenuRadioGroup` + `MenuRadioItem`, `MenuGroup` + `MenuGroupLabel`, `MenuSeparator`, `MenuShortcut`, `MenuSub` + `MenuSubTrigger` + `MenuSubPopup`. Modal: arrows, Home/End and typeahead move the highlight, Enter or Space picks, Escape and Tab close it, focus returns to the trigger, and a press outside only closes it. Items are 28px; disabled items stay focusable (`aria-disabled`) so a reason can be shown |
| `ScrollArea` | `orientation` vertical · horizontal · both, `fade` (24px edge fades, default true), `viewportClassName`, `viewportRef`, `viewportTabIndex`. Overlay 6px scrollbars that take no width and show while hovering or scrolling; it fills its parent |
| `Collapsible` | `Collapsible` (root: `open`, `defaultOpen`, `onOpenChange`), `CollapsibleTrigger` (`render` a row or button), `CollapsiblePanel` (`animate`: 200ms height; `hiddenUntilFound` and `keepMounted` default true, so find-in-page reaches and opens collapsed text) |
| `IconBadge` | `icon`, `tone` neutral · accent, `size` md · lg |
| `ProgressBar` | `label`, `value`, `max`, `showCount` |
| `Divider` | optional `label` (e.g. "or") |
| `Kbd`, `Avatar` | keyboard hint; initials avatar (`name`, `size`) |
| `Card` | `padding` md · lg |
| `Stack` | `direction`, `gap` (0–12, ×4px), `align`, `justify`, `wrap` |
| `Grid` | `columns` 2 · 3 · 4 (collapses by container width), `gap` |
| `Split` | two columns: `emphasis` equal · start · end, `divided`; stacks when narrow |
| `Container` | `size` xs · sm · prose · md · lg, `verticallyCentered` |
| `Heading`, `Text` | `Heading`: `level`, `size` display · title, `align`. `Text`: `as`, `variant` body · lead · label, `tone`, `align`, `measure` |

### Components

| Component | Key props |
| --- | --- |
| `PageHeader` | `title`, `description`, `eyebrow`, `actions`, `meta` (status line), `back: { label, onClick }`, `titleRef` |
| `SectionHeader` | `title`, `level` 2 · 3, `meta`, `actions` |
| `Brand` | `name`, `size` md · lg · xl, `layout` inline · stacked |
| `Composer` | `value`, `onValueChange`, `onSubmit`, `onAttach(openFilePicker)`, `label`, `placeholder`, `attachments`, `toolbarStart`, `toolbarEnd`, `size` default · compact, `textareaRef`, `ref` (the form; a `CommandMenu` anchor), `onKeyDown` (runs first; `preventDefault()` stops Enter from sending), `onSelectionChange(caret)`, `suggestions: { listId, open, activeId }` (field ARIA for a `CommandMenu`), `menu` (slot for the popup). Files: `onFiles(files, source)`, `accept`, `attachActions`, `uploadLabel`, `attachDisabledReason`, `dropLabel`, `onLargePaste(text)`, `largePasteBytes`. Sending: `canSubmitEmpty`, `submitBlockedReason`. See [Command menu](#command-menu) and [Attachments](#attachments) |
| `CommandMenu` | `open`, `anchorRef`, `inputRef`, `query`, `sections: { id, label?, items }[]`, `activeId`, `onActiveChange`, `onSelect`, `onClose`, `id`, `label`, `loading`, `loadingText`, `emptyText`, `footer`, `placement` (top-start), `matchAnchorWidth` (true), `container`. Items: `{ id, title, prefix?, description?, icon?, badge?: string · { label, tone?, icon? }, disabled?, tooltip? }`. See [Command menu](#command-menu) |
| `Dialog` | `open`, `title`, `onClose`, `description` (linked as the accessible description), `size` md · lg · xl (512 / 720 / 960px, clamped to the window less 32px), `footer` (actions pinned under the body), `initialFocusRef`, `dismissible` (default true: Escape and a backdrop press close it), `keepMounted` (default false). Base UI modal: traps focus, locks page scroll, and closes on Escape or a press on the backdrop only (not on its own padding, nor when a text selection is released outside). Focus starts at `initialFocusRef`, else stays where the content put it (`autoFocus`, or an owner effect that runs as `open` turns true), else goes to the first `[data-autofocus]` or focusable element in the body, then to the body itself when it is only text that scrolls (it is then a Tab stop, so arrow and Page keys scroll it), then the footer, then Close; the Close button comes after the body in Tab order. Focus returns to the opener. The header and footer stay put while the body scrolls in a `ScrollArea`. The content mounts as it opens and unmounts after the exit transition (pass `keepMounted` to keep its state). Popovers, menus, selects, tooltips and toasts opened inside sit above it. File drags over it never reach a drop target behind it (`containFileDrag`), and a `Composer` inside it takes its own drops |
| `ModelPicker` | `providers`, `models`, `value` (model key), `onValueChange`, `favorites: ReadonlySet<string>`, `onToggleFavorite`, `open`, `onOpenChange`, `anchorRef`, `label`, `onOpenProviderSetup`, `onManageProviders`, `emptyText`, `loading`, `jumpModifierLabel` ("⌘" or "Ctrl"), `placement`, `container`, `id`. See [Model picker](#model-picker) |
| `ModelPickerTrigger` | `provider`, `model`, `placeholder`, `open`, `onToggle`, `size` sm · md, `shortcutLabel` (appended to the tooltip), `disabled`, `ref` (pass it as the picker's `anchorRef`) |
| `Notice` | `tone` info · success · warning · danger, `variant` plain · banner (pass `role="alert"` for errors) |
| `MediaItem` | `media`, `title`, `description`, `trailing`, `titleLevel`, `size` md · lg |
| `InfoCard` | `icon`, `status`, `label`, `value`, `detail` |
| `ToastProvider` + `useToastManager` | App-wide toasts on Base UI Toast: `timeout` (5000ms), `limit` (3), `dismissLabel`. `useToastManager()` gives `notify(message, tone?)` (returns the id) and `dismiss(id)`. Toasts pause while hovered or focused and while the window is in the background; `danger` ones are announced assertively; F6 moves focus to them; a swipe right or down dismisses one. They sit above dialogs, and pressing one never closes a popover or dialog |
| `ToastRegion` | `toasts: { id, message, tone? }[]`, `onDismiss`, `dismissLabel`: the same toasts for an owner that keeps the list and the timing itself |
| `List` + `ListRow` | `ListRow`: `media`, `title`, `subtitle`, `meta: ReactNode[]`, `trailing`, `selected`, `onSelect` (whole row becomes a button) |
| `KeyValueList` | `items: { label, value }[]` |
| `SettingRow` | `title`, `description`, `control`, `controlId` |
| `SettingsSection` | `title`, `icon`, `description`, `actions`, `titleLevel` 2 · 3 · 4; bordered, titled group whose children stack with a gap |
| `ProviderRow` | `brand`, `title`, `version`, `headline`, `detail`, `tone` success · warning · danger · neutral (dot only for warning and danger), `selected`, `disabled` (dims, stays selectable), `badges`, `trailing` (outside the select button), `onSelect`. An `<li>` for a `List` |
| `ProviderTile` | `brand`, `name`, `description` (one line), `badges`, `selected` (`aria-pressed`; leave undefined for a plain action), `onSelect`, `disabled`; compact catalog button |
| `EmptyState` | `icon`, `title`, `description`, `actions`, `titleLevel` |
| `StatTile` | `label`, `value`, `hint`, `icon`, `tone` |
| `Meter` | `label`, `value`, `max`, `valueText`, `tone`, `warnAt`, `dangerAt` |
| `SegmentedMeter` | `label`, `segments: { label, value, color? }[]`, `max`, `valueText`, `remainderLabel` |
| `BarChart` | `caption`, `hideCaption`, `data: { label, value, highlight? }[]`, `unit`, `max` |
| `DataTable` | `columns: { key, header, align?, width?, rowHeader?, render? }[]`, `rows`, `rowKey`, `caption`, `hideCaption`, `emptyText` |
| `EntityCard` | `media`, `title`, `subtitle`, `badges`, `description`, `footer`, `selected`, `onSelect` |
| `ArtifactCard` | `name`, `kind` document · code · data · diagram · plan · web_page · other (picks the icon and badge; unknown kinds read as "File"), `kindLabel`, `icon`, `size` (bytes) or `sizeLabel`, `createdAt` or `createdLabel`, `detail` (e.g. language or MIME type), `onOpen` (the name becomes a button), `actions`, `titleLevel` |
| `PlanCard` | `markdown`, `title` (default: the plan's first heading, else "Proposed plan"), `status` active · superseded · completed, `actions` (footer, e.g. Implement), `headerActions`, `onOpenLink`, `codeBlockActions`, `collapseAfterLines` (20), `previewLines` (10), `expanded` / `defaultExpanded` / `onExpandedChange`, `titleLevel`. See [Plans and artifacts](#plans-and-artifacts) |
| `StepList` | `steps: { title, description?, status: done · active · pending · failed }[]` |
| `ActivityLog` | `entries: { time, text, tone? }[]`, `live` |
| `TaskGraph` | `root`, `nodes: { id, label, icon?, status?, detail? }[]` |
| `CodeDiff` | `fileName`, `language?`, either `oldCode` + `newCode` or a unified `patch`, `layout` unified · split (or `defaultLayout` + `onLayoutChange`), `showLayoutToggle`, `actions`. See [Code diffs](#code-diffs). `getDiffStats(fileName, source)` returns the same `{ additions, deletions }` the header shows |
| `CodeBlock`, `Terminal`, `InlineCode` | `CodeBlock`: `code`, `title`, `language`, `actions`. `Terminal`: `lines: { kind: command · output · error, text }[]`, `title` |
| `Markdown` | `source`, `onOpenLink(url)`, `headingLevelOffset`, `breaks`, `codeBlockActions(block)`, `renderText(text)`. Safe subset, no raw HTML; see [Markdown](#markdown) |
| `InlineChip` | `label`, `icon`, `tooltip`, `kindLabel` (spoken prefix, e.g. "Skill"), `tone` accent · neutral, `unresolved` (dashed), `rawText` (`data-raw-text`). Non-interactive pill for `$skill` tokens; `renderSkillTokens(text, render)` swaps `$name` mentions for chips |
| `WorkLogRow` | `icon`, `label`, `secondary` (muted, one line), `status` running · completed · failed (spinner; danger dot plus "Failed"), `statusText`, `trailing` (outside the toggle), `children` (expandable detail; the header becomes an `aria-expanded` button), `expanded` / `defaultExpanded` / `onExpandedChange` |
| `WorkLogGroup` | Same header props as `WorkLogRow`; `children` are the member rows, listed indented when expanded. `webSearchSummary(n)` gives "Searched the web" / "Searched the web N times" |
| `WebResultList` | `results: { title?, url, snippet?, published? }[]`, `onOpenLink(url)`, `openHint` (spoken after each title; "opens in your browser"), `emptyText`. Titles open through `onOpenLink`; unsafe URLs show as text |
| `ChatMessage` | `role` user · assistant, `meta`, `actions` |
| `TranscriptDivider` | `label` (e.g. "Continued as Agent"), `details: ReactNode[]` (joined with middle dots, e.g. project and permission), `icon`; a centered line of text between turns, read as ordinary text |
| `AttachmentTray` | `items: { id, name, kind, size?, previewUrl?, status? pending · uploading · ready · failed, progress? (0–1), error?, statusText? }[]`, `onRemove`, `onRetry`, `onPreview` (default: a built-in `ImageLightbox`), `onOpen`, `disabled`, `label`. Not a root-element passthrough. See [Attachments](#attachments) |
| `MessageAttachments` | `items: { id, name, kind, size?, pages?, src?, status? loading · ready · error, note? }[]`, `onRequestImage(id)`, `onImageVisibilityChange(id, visible)`, `onDownload(id)`, `label`. Not a root-element passthrough |
| `ImageLightbox` | `open`, `images: { name, src?, loading? }[]`, `index`, `onIndexChange`, `onClose`, `label` ("Image preview"). A full-window Base UI dialog above other dialogs (`--aidex-z-dialog-media`). Not a root-element passthrough |
| `FileDropZone` | `onFiles(files)`, `label` ("Drop files to attach"), `disabled`, `disabledReason`, `children`. Adds no box (`display: contents`). Not a root-element passthrough |
| `FileTree` | `nodes: { id, name, kind: folder · file, children?, meta? }[]`, `selectedId`, `onSelect`, `defaultExpanded` |
| `Timeline` | `items: { id, title, time, description? }[]`, `selectedId`, `onSelect` |
| `QrCodePattern` | `seed`, `size` (decorative stand-in; encodes no data) |

### Desktop

| Component | Key props |
| --- | --- |
| `AppShell` | `rail`, `sidebar`, `header`, `contentRef` |
| `NavRail` | `label`, `items: { id, label, icon }[]`, `activeId`, `onSelect` |
| `Sidebar`, `SidebarSection`, `SidebarItem` | `SidebarItem`: `label`, `icon`, `iconTone`, `active`, `size` md · sm, `count`, `actions`, `onSelect` |
| `TopBar` | `start`, `center`, `end` |
| `PageFrame` | `width` narrow · default · wide; the page body inside `AppShell` |
| `DockLayout` | `dock` (pinned to the bottom of the scroll area), `width` prose · default |
| `FocusLayout` | full-window frame for launch and setup flows |

### Installer

| Component | Key props |
| --- | --- |
| `SetupWindow` | `title`, `subtitle`, `badge`, `steps` (a `StepTrack`), `footer` (a `SetupFooter`), `contentRef`; centered window with title bar, step track, scrolling content column, pinned footer |
| `SetupFooter` | `note` (polite live text), `noteId`, action buttons as children |
| `StepTrack` | `label`, `steps: { id, title, status: complete · current · upcoming, reachable? }[]`, `onSelect` |

### Icons

| Export | Notes |
| --- | --- |
| Stroke icons | `createIcon` 24×24 strokes (`size`, `title`), e.g. `StarIcon` and `StarFilledIcon` for favorites, `ImageIcon` for linked (never loaded) images, `ZoomInIcon` and `ZoomOutIcon` for the image viewer, `ArrowRightLeftIcon` for a transcript divider |
| `BrandIcon` | `brand` (glyph key), `size` (20), `tone` brand · mono, `title`; other SVG props pass through. See [Brand icons](#brand-icons) |
| `hasBrandIcon(brand)`, `brandTitle(brand)`, `monogram(label)`, `type BrandId` | glyph lookup, display name, and the fallback initials |

## Overlays and layers

- **Stacking.** Tokens set the order: `--aidex-z-overlay` 40 (drop overlays), `--aidex-z-sidebar-overlay` 45, `--aidex-z-dialog` 50, `--aidex-z-dialog-media` 60 (the image viewer), `--aidex-z-toast` 100, `--aidex-z-popover` 130 (popovers, menus, selects, `CommandMenu`, `ModelPicker`), `--aidex-z-tooltip` 140. A menu, select, tooltip or toast opened over a dialog is always on top and usable.
- **Layer stack.** Open dismissable surfaces register in one stack (`pushLayer(element, { modal })` returns the removal; `popLayer`, `topLayer`, `isTopLayer`). Only the top layer answers Escape, and a press inside a layer opened on top of another, or inside an element marked `data-aidex-layer` (the toast region, menu and select portals, the dialog popup), is not outside the one below (`isInsideDescendantLayer(target, element)`). `hasModalLayer()` tells shell shortcuts such as Mod+B to stand down while a dialog is open.
- **Portal roots.** The `Dialog` popup carries `data-aidex-portal-root`, and a `Popover` opened inside it portals there, so it stays inside the focus trap. A portal root never has a transform, filter or containment at rest (they would offset fixed-position children); dialogs scale only while entering and leaving.
- **Motion.** Popups enter and leave over 150–200ms (`data-starting-style` / `data-ending-style`) and unmount after leaving; with reduced motion (the OS setting or `html[data-reduce-motion="true"]`) they appear and go at once.

## Brand icons

`BrandIcon` draws provider and agent-runtime logos from `icons/brands/brandGlyphs.ts`: generated, monochrome 24×24 path data (49 brands) from `@lobehub/icons-static-svg`, MIT licensed; see `icons/brands/NOTICE.md`. Do not edit the data by hand. Brand names and logos identify the provider only.

- **Color.** `tone="brand"` (default) colors the glyph with its brand color through the `--aidex-brand-color` custom property; glyphs without a usable color, and `tone="mono"`, inherit `currentColor`. In the light theme the brand color is darkened (`color-mix(in oklab, <brand> 78%, black)`, through the `--aidex-brand-color-strength` token) to keep contrast on light surfaces.
- **Fallback.** A key without a glyph renders a rounded-square monogram of up to two initials from `title`, or from `brandTitle(brand)` when there is no title (`monogram("llama.cpp server")` is "LS"; a single word gives its first two letters).
- **Accessibility.** Without `title` the icon is decorative (`aria-hidden`); with one it is `role="img"` with that name. Next to a visible provider name, leave `title` off.

## Model picker

`ModelPicker` is data only: the app maps its providers and models to `ModelPickerProvider` (`id`, `name`, `brand`, `status` ready · warning · error · disabled · setup, `statusText`, `kind` model · runtime · local, `setupLabel`) and `ModelPickerModel` (`key`, `providerId`, `name`, `shortName`, `description`, `contextWindow`, `capabilities` tools · vision · json · reasoning, `badge` new · default · custom, `unavailable`, `disabledReason`, `legacy`), and persists selection and favorites itself.

- **Layout.** A popover (380px wide, up to 420px tall) with a 44px provider rail (Favorites, then providers; agent runtimes after a divider) and a search field above a listbox. Each rail button shows a status dot; its tooltip and accessible name name any non-ready state ("Groq — Unavailable. 401 Unauthorized").
- **Views.** It opens on Favorites when any listed model is a favorite, otherwise on the selected model's provider (or the first ready one). Favorites are ordered by rail order. A provider view lists favorites, then current models, then a "Legacy models" row that expands the `legacy` ones. Typing searches every provider: tokens are matched against name, short name, provider name, and key (exact, prefix, word boundary, substring, then fuzzy for tokens of three or more characters), favorites are boosted, and ties sort by name. The rail hides while searching.
- **Footer.** A provider that is not ready (or has no models) shows its `statusText` or a fallback with an "Open provider setup" button when `onOpenProviderSetup` is set. `onManageProviders` adds a "Manage providers" bar.
- **Keyboard.** Focus starts in the search field. ArrowUp/ArrowDown move the active option (`aria-activedescendant`), Enter selects it (or toggles the legacy row), Escape closes and returns focus to the trigger. ArrowLeft on an empty query or Shift+Tab moves to the rail, where ArrowUp/ArrowDown/Home/End move, Enter or Space selects, and ArrowRight or typing returns to search. Tab from search reaches the active row's favorite button. While open, Mod+1…Mod+9 choose the first nine enabled rows (hints show when `jumpModifierLabel` is set) and Mod+Shift+ArrowUp/ArrowDown switch provider, skipping disabled ones. Rows with a `disabledReason` are skipped and show the reason as their tooltip.
- **Search helpers.** `rankBySearch(items, query, toSearchable)` and `scoreModelSearch(model, query)` expose the same ranking, for example to filter a model list elsewhere; `formatContextWindow(tokens)` gives "200K" / "1M".

## Attachments

Files enter through the `Composer` and render with `AttachmentTray` (before sending) and `MessageAttachments` (in a sent message). The components only display and collect files: the app validates, uploads, and decides what a kind is (`image`, `pdf`, `text`; any other kind reads as "File").

- **Intake.** With `onFiles`, the attach button opens a hidden `<input type="file" multiple>` (filtered by `accept`, reset after every pick). `attachActions` (for example "From project…") turn the button into a menu (`ComposerAttachMenu`, on `Menu`: ArrowDown or ArrowUp on the button opens it on the first or last item, typeahead moves the highlight, Tab closes it and moves on, and an action's `disabledReason` shows as its tooltip) that starts with "Upload from computer" (`uploadLabel`); `onAttach(openFilePicker)` replaces all of that with the app's own action. Pasted files become files when the clipboard has no plain text (blank text counts as none only when there is no HTML either). With text, only an image copied on its own does, that is when the clipboard's HTML is just the image (a browser's "Copy image"); everything else pastes as text, including rich copies that also carry a bitmap, such as cells from Excel (even empty ones) or a OneNote page (`shouldClaimFilePaste(files, plainText, html)`, a single linear pass that treats unclosed markup as text). A plain-text paste of `largePasteBytes` (32 KiB) or more, in characters or UTF-8 bytes, goes to `onLargePaste` instead of the field, unless the user pressed Ctrl/Cmd+Shift+V. `source` tells the app whether files came from `picker`, `paste`, or `drop`.
- **Dropping.** Dragging files over a composer shows a dashed overlay with a paperclip and `dropLabel`. `FileDropZone` makes a larger region, such as a whole conversation page, the drop target, with the same overlay over its visible part; a `Composer` inside a zone leaves drops to the zone, so files are never added twice. Do not nest zones. A modal over the zone (`Dialog`, `ImageLightbox`) keeps file drags to itself, so a file dropped on it never lands in the composer behind it; `containFileDrag` does this for other modal surfaces, and a drop target nested inside the modal still accepts its drags. The highlight follows a `dragenter`/`dragleave` count and resets on a drop anywhere, `dragend`, or Escape. Dropped folders arrive as unreadable `File` entries for the app to refuse.
- **Window guard.** Call `useFileDropGuard()` once at the app root (or `installFileDropGuard(window)`). Drop targets accept a drag by cancelling `dragover`; the guard refuses every other file drag (`dropEffect = "none"`), refuses links dropped outside a text field, and cancels every file drop, so a file dropped anywhere else never navigates the window. In Tauri this matters once `dragDropEnabled` is off, because the webview then handles drops itself.
- **Blocking.** `attachDisabledReason` disables the attach button, file paste, and drops, and shows the reason as the attach tooltip and in the drop overlay. `submitBlockedReason` keeps send unavailable (it stays focusable with `aria-disabled`, so the reason can be read) and shows the reason as its tooltip and as a hint under the composer. `canSubmitEmpty` allows sending blank text, for example when only attachments are ready.
- **Tray.** Images are 64px tiles: a preview button (`Preview <name>`, opens an `ImageLightbox` over the tray's images), an X (`Remove <name>`), a `NN%` progress strip while uploading (a `progressbar`), and, for a failed upload, a danger border plus a retry button (`Retry upload for <name>`) whose tooltip and description give the reason. Other files are rows with a kind icon, name, status (`Text · 48 KB`, `NN%`, "Upload failed", or `statusText`), retry, and X. Failures are announced in a polite live region. `formatAttachmentSize` gives "3.2 MB" from 1 MiB up and whole kilobytes below, never "0 KB".
- **Sent messages.** `MessageAttachments` goes in a `ChatMessage` bubble above the text: images in a two-column grid of 4:3 tiles at most 210px wide (a single image spans both columns), with a placeholder showing the name while loading or after a failure; files as rows with kind, page count, size, an optional `note`, and a Download button (`status: "loading"` shows a spinner while downloading). Tiles without `src` (and not `loading` or `error`) call `onRequestImage(id)` as they scroll into view, or at once when they lose `src` while in view (one `IntersectionObserver` per component watches every tile, with a 200px margin), and the viewer asks for the image it shows when the grid has not; the call can repeat for an id, so cache by id. `onImageVisibilityChange(id, visible)` reports when an image comes on screen (its tile in view, or the viewer showing it) and when it leaves, and reports `false` for every image still on screen when the component unmounts, so an app can keep on-screen images out of cache eviction (otherwise an evicted image in view is asked for again, which can loop).
- **Viewer.** `ImageLightbox` is a full-window modal (Base UI Dialog) above other dialogs. The close button takes focus on open (without showing its tooltip); one Escape or a click outside the image closes it, and focus returns to the opener. Prev/Next and ArrowLeft/ArrowRight wrap around, and the caption reads `<name> (i/n)` politely. Zoom runs from fit to 8x: a click on the image, or the zoom button (`aria-pressed`), toggles between fit and 2x around the clicked point; the wheel zooms around the pointer; `+`, `-` and `0` zoom in, out, and back to fit. While zoomed, dragging or the arrow keys pan. A missing or failed image shows "Image unavailable." ("Loading image…" while `loading`).
- **Data URLs only.** Thumbnails and the viewer display `data:image/...` URLs and ignore anything else (`safeImageDataUrl`): the desktop release CSP allows `data:` images but not `blob:`, and a preview must never fetch. Images are not draggable, so a thumbnail cannot be dropped back in as a new file.
- **Theming and motion.** Everything uses the `--aidex-*` tokens in both themes; the overlay fade, progress fill, viewer fade, and spinners stop under reduced motion.

## Markdown

`Markdown` renders model output, skill bodies, plans, and markdown artifacts with a dependency-free parser (`parseMarkdown(source)` returns the block tree; `markdownInlineText(nodes)` flattens inline nodes to text). Both are pure, so they run during static export and in unit tests.

- **Syntax.** ATX and setext headings, paragraphs, `*emphasis*`, `**strong**`, `~~strikethrough~~`, code spans, fenced code (rendered with `CodeBlock`, language from the fence), ordered, unordered, nested, and task lists, blockquotes, GFM tables with alignment, horizontal rules, inline and reference links, `<autolinks>`, bare `https://` and `www.` links, images, hard breaks, backslash escapes, and common entities.
- **Streaming.** An unclosed fence runs to the end of the text and renders as code (`data-streaming="true"` on its `CodeBlock`), so a response that is still arriving never flashes as prose.
- **Safety.** Raw HTML is never interpreted: `<script>` or `<img onerror>` stays literal text. Links and images keep only absolute `http(s)` URLs (links also `mailto:`) without embedded credentials, checked by `safeExternalUrl` (exported with `urlHost`); `javascript:`, `data:`, `file:`, relative, and obfuscated schemes become inert text. Links have no `href`, so the webview can never navigate: activating one (click or Enter) calls `onOpenLink(url)`, and the URL is the tooltip. Without `onOpenLink`, links render as plain text. Images are never loaded; they show as an image chip that opens like a link, or an inert chip inside a link's text (a README badge such as `[![CI](badge.svg)](ci)`). Nesting depth and delimiter runs are bounded, so hostile input cannot exhaust the stack, and parsing stays linear on long whitespace runs, deep bracket nests, and unclosed link titles, so it cannot freeze the UI.
- **Headings and chips.** `headingLevelOffset` shifts heading tags under the surrounding outline while keeping their look. `renderText` maps plain text runs (never code or link text), for example `(text) => renderSkillTokens(text, toChip)` to show `$skill` chips.

## Command menu

`CommandMenu` is the composer's `$` skills menu and `/` command menu: a `Popover` above the composer, sized to it, holding a listbox of sections. It is fully controlled; the app detects the trigger, filters and ranks `sections` from `query`, and inserts the pick.

- **Focus stays in the text field.** Pass the composer's `textareaRef` as `inputRef` and the composer `ref` as `anchorRef`, and give the composer `suggestions={{ listId, open, activeId }}` (or spread `commandMenuFieldProps(...)` onto another field) so the field carries `aria-controls` and `aria-activedescendant` (option ids come from `commandMenuOptionId(listId, itemId)`).
- **Keyboard.** While open, ArrowUp/ArrowDown in the field move the active row through every row, disabled ones included (wrapping), Enter or Tab selects it, and Escape closes. These keys never reach the field, so Enter does not send. On a disabled row Enter and Tab do nothing and the menu stays open: no send, no other row, no focus move. With no active row, Enter and Tab behave as usual; Shift+Enter, Shift+Tab, and presses with Alt, Ctrl, or Meta always reach the field. Opening, and every change of `query`, highlights the first row even when it is disabled, so list the best match first: Enter never runs a lower row in its place. After that the highlight stays on its row while the row is listed, and otherwise moves to the first row. `resolveCommandMenuActive` and `commandMenuKeyAction` expose the same logic.
- **Pointer.** Hovering highlights a row (disabled ones too) and clicking selects it without taking focus from the field; clicking a disabled row does nothing. A press outside, or focus moving elsewhere in the page, calls `onClose`.
- **States.** A disabled row is dimmed, keeps the full highlight when active, and carries `aria-disabled`, so the field's `aria-activedescendant` announces it as unavailable. `loading` with no rows shows `loadingText` with a spinner; no rows shows `emptyText` (t3code copy: "No skills found. Try / to browse provider commands." for skills, "No matching command." otherwise). A polite live region announces the result count, with the unavailable rows counted separately ("3 results, 1 unavailable"). The first match of `query` in each title is bold and underlined.
- **Mentions.** `splitSkillTokens(text)` and `findSkillMentions(text)` find `$name` tokens (letters, digits, `_`, `-`, at least one letter; amounts such as `$20`, `$20k`, `$1.5` are not mentions).

## Plans and artifacts

- **`PlanCard`.** The plan's first heading becomes the title (a "Summary" heading right after it is dropped) and the rest renders with `Markdown` under it. Plans over about 20 lines (long lines count once per 100 characters) show a 10-line preview that fades out, with a "Show full plan" / "Show less" toggle (`aria-expanded`). `splitPlanMarkdown`, `planLineCount`, and `planPreview` expose the same logic.
- **`ArtifactCard`.** `formatByteSize` gives "1.5 KB"; `createdAt` is shown with `formatDateTime` in the viewer's locale inside a `<time>` element. `artifactKindLabel(kind)` gives the badge text; any kind that is not one of the seven, including names such as `constructor`, reads as "File".

## Code diffs

`CodeDiff` is the one diff component; every diff in an app renders through it. It wraps [`@pierre/diffs`](https://diffs.com) (Shiki-based) and adds an accessible header: file name, `+N −M` counts with a spoken "N additions, M deletions" summary, a unified/split `SegmentedControl`, and an `actions` slot.

- **Client only, lazy.** The header and a placeholder sized to the diff are prerendered (safe under `output: "export"`). After hydration the highlighter chunk (`CodeDiffRenderer`) loads with `React.lazy`; Shiki themes and grammars are further split into on-demand chunks, so a screen without diffs never downloads them.
- **No WebAssembly, no network.** Shiki uses its JavaScript regex engine (`preferredHighlighter: "shiki-js"`), so the Tauri CSP needs no `wasm-unsafe-eval`. Themes and grammars are bundled; nothing is fetched at runtime.
- **Theming.** Syntax colors come from the bundled `pierre-dark` theme. Surface, text, line tints, and fonts map onto `--aidex-*` tokens through the `--aidex-color-diff-*` tokens in `tokens.css`, set as `--diffs-*` custom properties on the library's host element.
- **Accessibility.** Lines carry classic `+`/`-` markers, so changes never rely on color alone. Long lines wrap instead of scrolling, and the library's mouse-only hunk-expand controls are turned off.

## Checks

```sh
bun run --cwd packages/ui check
```
