# UI primitives

Small accessible controls and layout primitives shared across frontends:

- Actions: `Button`, `IconButton`, `Kbd`.
- Overlays: `Popover` (anchored, non-modal, portalled), and on Base UI `Tooltip` with `TooltipProvider` and `Menu` with its parts. `layers.ts` holds the stack of open layers (top-first Escape and outside presses, `hasModalLayer`, `layerCount`, and the push listener a popover uses to close when a modal opens over it), the tabbable helpers behind the popover's focus guards, and the focus-intent check (`focusFollowsNavigationKey`) that keeps focus placed by a script from opening a tooltip.
- Scrolling and disclosure: `ScrollArea` (overlay scrollbars, edge fades) and `Collapsible` (animated, reachable by find-in-page), on Base UI.
- Form fields: `TextField`, `SearchField`, `PasswordField`, `TextArea`, `Select` (Base UI, with a native-select change event from `selectEvent.ts`), `Checkbox`, `Switch`, `SegmentedControl`, `ChoiceCardGroup`, `SwatchPicker`. Labels, hints, and errors share `field.module.css`.
- Navigation: `Tabs` and `TabPanel`.
- Display: `Avatar`, `Badge`, `StatusDot`, `StatusIndicator`, `IconBadge`, `ProgressBar`, `Divider`, `Card`.
- Layout and type: `Stack`, `Grid`, `Split`, `Container`, `Heading`, `Text`. `Stack` and `Grid` share the token gap scale in `gap.module.css`.

Each component keeps its styles in a sibling CSS Module.
